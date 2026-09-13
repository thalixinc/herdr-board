//! Startup sweep and human reconciliation: surface stale `handed-off` receipts
//! and re-dispatch, answer, or cancel them idempotently.

use std::time::Duration;

use crate::digest::compute_with_version;
use crate::outbox::{Outcome, Receipt, ReceiptId, Store, StoreError};

use super::{reconstruct_request, HandoffResult, HandoffTransport, ReceiverError};

/// A `handed-off` receipt older than this is surfaced as "outcome unknown".
pub const STALENESS_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// Surface non-terminal receipts that need attention: `handed-off` receipts
/// older than [`STALENESS_THRESHOLD`], oldest first. Never re-dispatches.
///
/// `now` is a unix-seconds timestamp (matching `Receipt::created_at`). A store
/// read failure yields an empty sweep (best effort — nothing is surfaced).
pub fn startup_sweep(store: &Store, now: i64) -> Vec<Receipt> {
    let threshold = STALENESS_THRESHOLD.as_secs() as i64;
    store
        .list_non_terminal()
        .unwrap_or_default()
        .into_iter()
        .filter(|receipt| {
            receipt.outcome == Outcome::HandedOff && now - receipt.created_at >= threshold
        })
        .collect()
}

/// Re-confirm a receipt: re-verify its digest against the persisted record,
/// then re-dispatch idempotently. A terminal receipt refuses the transition
/// (never double-accepted).
pub fn reconfirm(
    store: &Store,
    receipt_id: &ReceiptId,
    transport: &impl HandoffTransport,
) -> Result<Receipt, ReceiverError> {
    let receipt = store
        .receipt_by_id(receipt_id)
        .map_err(ReceiverError::Store)?
        .ok_or_else(|| ReceiverError::Store(StoreError::NotFound))?;

    let stored_fields = reconstruct_request(store, &receipt)?;
    let request = stored_fields.to_request();
    // The persisted record must still match the receipt's digest, under the
    // record's own schema version; otherwise refuse rather than re-dispatching
    // a drifted request.
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

/// Record an out-of-band external answer on a receipt and transition it. The
/// board stays the authority: the answer is stored verbatim and only ever
/// transitions the receipt — never rewrites the digest or the persisted
/// request.
pub fn status_query(
    store: &Store,
    receipt_id: &ReceiptId,
    accepted: bool,
    response: &str,
) -> Result<Receipt, ReceiverError> {
    let outcome = if accepted {
        Outcome::Accepted
    } else {
        Outcome::Refused
    };
    store
        .transition_to(receipt_id, outcome, Some(response))
        .map_err(ReceiverError::Store)
}

/// Human-gated terminal cancel: mark a receipt `cancelled`.
pub fn cancel_receipt(store: &Store, receipt_id: &ReceiptId) -> Result<Receipt, ReceiverError> {
    store
        .transition_to(receipt_id, Outcome::Cancelled, None)
        .map_err(ReceiverError::Store)
}

/// Replay a receipt: an idempotent no-op that returns the receipt unchanged.
/// Replaying never re-runs the handoff.
pub fn replay(store: &Store, receipt_id: &ReceiptId) -> Result<Receipt, ReceiverError> {
    store
        .receipt_by_id(receipt_id)
        .map_err(ReceiverError::Store)?
        .ok_or_else(|| ReceiverError::Store(StoreError::NotFound))
}
