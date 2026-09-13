//! Startup sweep and human reconciliation: surface stale `handed-off` receipts
//! and re-dispatch or answer them idempotently.

use std::time::Duration;

use crate::digest::compute_with_version;
use crate::outbox::{Outcome, Receipt, Store, StoreError};

use super::{
    reconstruct_request, ExternalResponse, HandoffResult, HandoffTransport, ReceiverError,
};

/// A `handed-off` receipt older than this is surfaced as "outcome unknown".
pub const STALENESS_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// Surface non-terminal receipts that need attention: `handed-off` receipts
/// older than [`STALENESS_THRESHOLD`], oldest first.
///
/// `now` is a unix-seconds timestamp (matching `Receipt::created_at`).
pub fn startup_sweep(store: &Store, now: i64) -> Result<Vec<Receipt>, ReceiverError> {
    let threshold = STALENESS_THRESHOLD.as_secs() as i64;
    let mut stale = Vec::new();
    for receipt in store.list_non_terminal().map_err(ReceiverError::Store)? {
        if receipt.outcome == Outcome::HandedOff && now - receipt.created_at >= threshold {
            stale.push(receipt);
        }
    }
    Ok(stale)
}

/// Re-confirm a `handed-off` receipt: re-dispatch the same digest + same
/// handoff.
///
/// Idempotent: one handoff id maps to one receipt, and a terminal receipt
/// refuses further transitions, so re-confirming can never double-accept.
pub fn reconfirm(
    store: &Store,
    receipt: &Receipt,
    transport: &impl HandoffTransport,
) -> Result<Receipt, ReceiverError> {
    let stored_fields = reconstruct_request(store, receipt)?;
    let request = stored_fields.to_request();
    // The persisted record must still match the receipt's digest, under the
    // record's own schema version; otherwise the board refuses rather than
    // re-dispatching a drifted request.
    if compute_with_version(&request, stored_fields.schema_version) != receipt.digest {
        return Err(ReceiverError::Store(StoreError::InvalidData(
            "persisted request does not match receipt digest".to_owned(),
        )));
    }
    match transport.handoff(&request) {
        HandoffResult::Accepted(response) => store.transition_to(
            &receipt.receipt_id,
            Outcome::Accepted,
            Some(response.as_str()),
        ),
        HandoffResult::Refused(response) => store.transition_to(
            &receipt.receipt_id,
            Outcome::Refused,
            Some(response.as_str()),
        ),
        HandoffResult::Failed(message) => return Err(ReceiverError::Transport(message)),
    }
    .map_err(ReceiverError::Store)
}

/// Record an out-of-band external answer on a `handed-off` receipt.
///
/// The board stays the authority: the answer is stored verbatim and only ever
/// transitions the receipt — it never rewrites the digest or the persisted
/// request.
pub fn status_query(
    store: &Store,
    receipt: &Receipt,
    accepted: bool,
    response: &ExternalResponse,
) -> Result<Receipt, ReceiverError> {
    let outcome = if accepted {
        Outcome::Accepted
    } else {
        Outcome::Refused
    };
    store
        .transition_to(&receipt.receipt_id, outcome, Some(response.as_str()))
        .map_err(ReceiverError::Store)
}

/// Replay a receipt: an idempotent no-op that returns the receipt unchanged.
/// Replaying never re-runs the handoff.
pub fn replay(store: &Store, receipt: &Receipt) -> Result<Receipt, ReceiverError> {
    store
        .get_by_handoff_id(&receipt.handoff_id)
        .map_err(ReceiverError::Store)?
        .ok_or_else(|| ReceiverError::Store(StoreError::NotFound))
}
