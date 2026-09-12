//! Receiver orchestration: one entry point that proves provenance, verifies
//! the G2 digest, dedups, and drives a handoff to a terminal outcome through a
//! three-transaction write-ahead.

mod actor;
mod handoff;
mod reconcile;

pub use actor::{prove_actor, Actor, ActorSource, TrustRoot};
pub use handoff::{ExternalResponse, Handoff, HandoffResult, HandoffTransport};
pub use reconcile::{reconfirm, replay, startup_sweep, status_query, STALENESS_THRESHOLD};

use std::fmt;

use crate::digest::{compute, verify, CanonicalRequest, Identity, Mismatch, StoredFields};
use crate::outbox::{Outcome, Receipt, RequestRecord, Store, StoreError};

/// The receiver boundary — a sealed marker type.
///
/// There is no public constructor: the single entry point is the free function
/// [`receive`]. `Receiver` names the boundary in type signatures and re-exports
/// without exposing any state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receiver {
    _sealed: (),
}

/// A failure while receiving a handoff. Each variant maps to a distinct,
/// observable refusal; none silently recompute or overwrite the stored digest.
#[derive(Debug)]
pub enum ReceiverError {
    /// The presented request's digest does not match the persisted digest —
    /// a field-level conflict (G2 [`Mismatch`]). The factory refuses to run.
    Refusal(Mismatch),
    /// The handoff's `actor` field does not match the board's proven trust root.
    ActorMismatch {
        /// The proven trust-root actor.
        proven: Actor,
        /// The actor claimed by the handoff.
        claimed: String,
    },
    /// A non-terminal (active) attempt already exists for this input/request.
    ActiveAttemptExists {
        /// The existing active receipt; the receiver never dispatches a second.
        receipt: Box<Receipt>,
    },
    /// The handoff transport failed; the receipt remains `handed-off`
    /// (outcome unknown) for later reconciliation.
    Transport(String),
    /// The outbox store failed.
    Store(StoreError),
}

impl fmt::Display for ReceiverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReceiverError::Refusal(_) => f.write_str("digest mismatch: factory refuses to run"),
            ReceiverError::ActorMismatch { proven, claimed } => {
                write!(
                    f,
                    "actor mismatch: proven {:?}, claimed {:?}",
                    proven.value, claimed
                )
            }
            ReceiverError::ActiveAttemptExists { .. } => {
                f.write_str("an active (non-terminal) attempt already exists")
            }
            ReceiverError::Transport(msg) => write!(f, "transport error: {msg}"),
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

/// The single entry point: receive a handoff and drive it to a terminal
/// outcome (or a refusal).
///
/// The write-ahead is three transactions: prove the actor, enforce the trust
/// root, verify the G2 digest, dedup in-flight input, acquire the
/// active-attempt slot (`pending`), commit `handed-off` before the external
/// call, then finalize to `accepted`/`refused`.
pub fn receive(
    store: &Store,
    handoff: &Handoff,
    transport: &impl HandoffTransport,
) -> Result<Receipt, ReceiverError> {
    let request = &handoff.request;

    // 1. Prove the actor from the trust root (env), never from the body.
    let actor = prove_actor();

    // 2. Enforce the trust root: the handoff must claim the proven actor.
    if request.actor.is_empty() || request.actor != actor.value {
        return Err(ReceiverError::ActorMismatch {
            proven: actor,
            claimed: request.actor.clone(),
        });
    }

    // 3. verify() (G2): a re-delivered handoff is checked against the persisted
    //    snapshot; any drift surfaces as a field-level Refusal(Mismatch).
    //    An identical re-delivery returns the existing receipt unchanged.
    if let Some(existing) = store
        .get_by_handoff_id(&handoff.handoff_id)
        .map_err(ReceiverError::Store)?
    {
        let stored_request = reconstruct_request(store, &existing)?;
        let stored_fields = StoredFields::from(&stored_request);
        if let Err(mismatch) = verify(&existing.digest, &stored_fields, request) {
            return Err(ReceiverError::Refusal(mismatch));
        }
        return Ok(existing);
    }

    // 4. Dedup: identical input already in flight?
    let digest = compute(request);
    if let Some(active) = store
        .get_active_by_digest(&digest)
        .map_err(ReceiverError::Store)?
    {
        return Err(ReceiverError::ActiveAttemptExists {
            receipt: Box::new(active),
        });
    }

    // 5. Acquire the active-attempt slot: persist + insert `pending`.
    let record = RequestRecord {
        digest,
        identity: request.identity.canonical(),
        revision: request.revision.clone(),
        factory: request.factory.clone(),
        actor: request.actor.clone(),
        actor_source: actor.source.as_str().to_owned(),
        body: request.body.clone(),
    };
    let pending = match store.insert_pending(&record, &handoff.handoff_id) {
        Ok(receipt) => receipt,
        Err(StoreError::ActiveAttemptExists) => {
            // A racing writer won; surface the active receipt.
            let active = store
                .get_active_by_digest(&digest)
                .map_err(ReceiverError::Store)?
                .ok_or_else(|| invalid("active attempt vanished".to_owned()))?;
            return Err(ReceiverError::ActiveAttemptExists {
                receipt: Box::new(active),
            });
        }
        Err(StoreError::DuplicateHandoff) => {
            // Handoff id reused concurrently; return the existing receipt.
            let existing = store
                .get_by_handoff_id(&handoff.handoff_id)
                .map_err(ReceiverError::Store)?
                .ok_or_else(|| invalid("duplicate handoff receipt vanished".to_owned()))?;
            return Ok(existing);
        }
        Err(other) => return Err(ReceiverError::Store(other)),
    };

    // 6. Commit `handed-off` before the external call (write-ahead).
    let handed_off = store
        .transition_to(&pending.receipt_id, Outcome::HandedOff, None)
        .map_err(ReceiverError::Store)?;

    // 7. Perform the one external handoff and finalize.
    match transport.handoff(request) {
        HandoffResult::Accepted(response) => store
            .transition_to(
                &handed_off.receipt_id,
                Outcome::Accepted,
                Some(response.as_str()),
            )
            .map_err(ReceiverError::Store),
        HandoffResult::Refused(response) => store
            .transition_to(
                &handed_off.receipt_id,
                Outcome::Refused,
                Some(response.as_str()),
            )
            .map_err(ReceiverError::Store),
        HandoffResult::Failed(message) => {
            // Outcome unknown: the receipt stays `handed-off` for reconciliation.
            Err(ReceiverError::Transport(message))
        }
    }
}

/// Reconstruct the [`CanonicalRequest`] a receipt was dispatched from, by
/// reading the persisted request record (which carries the `body` receipts
/// deliberately do not denormalize).
pub(crate) fn reconstruct_request(
    store: &Store,
    receipt: &Receipt,
) -> Result<CanonicalRequest, ReceiverError> {
    let record = store
        .get_request_by_id(&receipt.request_id)
        .map_err(ReceiverError::Store)?
        .ok_or_else(|| invalid("receipt references a missing request".to_owned()))?;
    Ok(CanonicalRequest {
        identity: parse_identity(&record.identity).ok_or_else(|| {
            invalid(format!(
                "cannot parse canonical identity {:?}",
                record.identity
            ))
        })?,
        revision: record.revision,
        factory: record.factory,
        actor: record.actor,
        body: record.body,
    })
}

/// Parse the canonical `owner/repo#number` form back into an [`Identity`].
///
/// GitHub owner/repo names cannot contain `/` or `#`, so the rightmost `#` and
/// `/` split unambiguously.
fn parse_identity(canonical: &str) -> Option<Identity> {
    let (owner_repo, number) = canonical.rsplit_once('#')?;
    let (owner, repo) = owner_repo.rsplit_once('/')?;
    Some(Identity::new(owner, repo, number.parse().ok()?))
}

fn invalid(message: String) -> ReceiverError {
    ReceiverError::Store(StoreError::InvalidData(message))
}
