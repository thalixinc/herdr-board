//! The receiver: the board's single, private, enforced seam every
//! "Process with factory" handoff flows through.
//!
//! [`receive`] proves the actor, verifies the G2 digest, dedups, acquires the
//! active-attempt slot, writes a durable receipt, performs the one external
//! handoff, and finalizes the receipt — returning a [`Receipt`] or a
//! [`ReceiverError`]. There is no other path from the explicit action to
//! coordinator/planner, and no background loop reaches it: sync never starts
//! work.

use crate::digest::{compute, verify, Identity, Mismatch, StoredFields};
use crate::outbox::{Outcome, Receipt, RequestRecord, Store, StoreError};

mod actor;
mod handoff;
mod reconcile;

pub use actor::{prove_actor, Actor, ActorSource, TrustRoot};
pub use handoff::{ExternalResponse, Handoff, HandoffResult, HandoffTransport};
pub use reconcile::{reconfirm, replay, startup_sweep, status_query, STALENESS_THRESHOLD};

/// The sealed receiver seam. Its single entrypoint is [`receive`]; the durable
/// state lives in the board's own outbox [`Store`], never a listening service
/// or shared queue.
pub struct Receiver;

/// The single entrypoint of the receiver.
///
/// Write-ahead order (each step committed before the next): persist a `pending`
/// receipt (acquiring the active-attempt slot) → flip to `handed-off` → perform
/// the external call → finalize to `accepted`/`refused`. A crash after the
/// `handed-off` commit leaves a durable, reconcilable "outcome unknown" row
/// rather than a silently lost handoff.
pub fn receive(
    store: &Store,
    handoff: &Handoff,
    transport: &impl HandoffTransport,
) -> Result<Receipt, ReceiverError> {
    // 1. Prove the actor from the process trust root.
    let trust_root = TrustRoot::from_env();
    let actor = prove_actor(&trust_root);

    // 2. Actor provenance: the request's claimed actor must match the trust
    //    root. A swapped/forged actor is refused before anything is persisted.
    if handoff.request.actor != trust_root.value {
        return Err(ReceiverError::ActorMismatch);
    }

    // 3. Compute the digest and build the durable request record.
    let digest = compute(&handoff.request);
    let record = RequestRecord {
        digest,
        identity: handoff.request.identity.canonical(),
        revision: handoff.request.revision.clone(),
        factory: handoff.request.factory.clone(),
        actor: actor.0.clone(),
        actor_source: trust_root.source.as_str().to_string(),
        body: handoff.request.body.clone(),
    };

    // 4. Idempotent re-delivery / drift: if this handoff_id was already
    //    persisted, verify the presented request against the stored one. A
    //    matching request re-delivers the existing receipt unchanged; a drifted
    //    request is refused.
    if let Some(existing) = store
        .get_by_handoff_id(&handoff.handoff_id)
        .map_err(ReceiverError::Store)?
    {
        if let Some(stored_record) = store
            .get_request_by_id(&existing.request_id)
            .map_err(ReceiverError::Store)?
        {
            let stored_fields = stored_fields_from_record(&stored_record)?;
            verify(&existing.digest, &stored_fields, &handoff.request)
                .map_err(ReceiverError::Refusal)?;
        }
        return Ok(existing);
    }

    // 5. Dedup: same input (same digest) already in-flight → converge on it.
    if let Some(active) = store
        .get_active_by_digest(&digest)
        .map_err(ReceiverError::Store)?
    {
        return Ok(active);
    }

    // 6. Acquire the active-attempt slot by inserting the pending receipt.
    let receipt = store
        .insert_pending(&record, &handoff.handoff_id)
        .map_err(|e| match e {
            StoreError::ActiveAttemptExists => ReceiverError::ActiveAttemptExists,
            other => ReceiverError::Store(other),
        })?;

    // 7. Commit handed-off before the external call.
    let handed = store
        .transition_to(&receipt.receipt_id, Outcome::HandedOff, None)
        .map_err(ReceiverError::Store)?;

    // 8. Perform the one external handoff.
    let result = transport.handoff(handoff);

    // 9. Finalize based on the external result.
    let outcome = if result.accepted {
        Outcome::Accepted
    } else {
        Outcome::Refused
    };
    let finalized = store
        .transition_to(
            &handed.receipt_id,
            outcome,
            Some(result.response.0.as_str()),
        )
        .map_err(ReceiverError::Store)?;

    Ok(finalized)
}

/// Reconstruct the persisted [`StoredFields`] snapshot from a stored request
/// record, re-canonicalizing the identity.
fn stored_fields_from_record(record: &RequestRecord) -> Result<StoredFields, ReceiverError> {
    let identity = parse_identity(&record.identity).ok_or_else(|| {
        ReceiverError::Store(StoreError::InvalidData(
            "stored identity is not canonical owner/repo#number".to_string(),
        ))
    })?;
    Ok(StoredFields {
        identity,
        revision: record.revision.clone(),
        factory: record.factory.clone(),
        actor: record.actor.clone(),
        body: record.body.clone(),
    })
}

fn parse_identity(canonical: &str) -> Option<Identity> {
    let (owner, rest) = canonical.split_once('/')?;
    let (repo, number) = rest.rsplit_once('#')?;
    Some(Identity::new(owner, repo, number.parse().ok()?))
}

/// Errors the receiver can return.
#[derive(Debug)]
pub enum ReceiverError {
    /// The handoff's claimed actor does not match the board's current
    /// trust-root identity (forged or swapped actor).
    ActorMismatch,
    /// The presented input drifted from the persisted request (G2 mismatch).
    Refusal(Mismatch),
    /// Another non-terminal attempt already exists for this request/digest.
    ActiveAttemptExists,
    /// The external handoff could not be performed.
    Transport(String),
    /// An underlying outbox-store error.
    Store(StoreError),
}

impl std::fmt::Display for ReceiverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReceiverError::ActorMismatch => {
                write!(f, "actor does not match the board's trust-root identity")
            }
            ReceiverError::Refusal(m) => write!(f, "refused: {m}"),
            ReceiverError::ActiveAttemptExists => {
                write!(f, "an active attempt already exists for this request")
            }
            ReceiverError::Transport(msg) => write!(f, "handoff transport failed: {msg}"),
            ReceiverError::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for ReceiverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReceiverError::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StoreError> for ReceiverError {
    fn from(e: StoreError) -> Self {
        ReceiverError::Store(e)
    }
}
