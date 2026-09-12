//! Reconciliation entrypoints: convert "uncertain" handoffs back to "decided"
//! without ever starting work. Every one of these is a read-only, idempotent
//! no-op that returns the existing receipt — re-dispatch and status queries are
//! human-gated actions built on [`super::receive`] and the store, never
//! auto-dispatched here.

use std::time::Duration;

use crate::outbox::{HandoffId, Receipt, Store};

use super::ReceiverError;

/// How long a `handed-off` receipt may sit without an ack before it is surfaced
/// as "pending, outcome unknown" in the startup sweep.
pub const STALENESS_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// Scan for non-terminal receipts on pane open. The returned rows are the
/// durable in-flight attempts (pending or handed-off); the board surfaces each
/// `handed-off` row past [`STALENESS_THRESHOLD`] as a visible "outcome unknown"
/// card. This surfaces — it never re-dispatches.
pub fn startup_sweep(store: &Store) -> Result<Vec<Receipt>, ReceiverError> {
    store.list_non_terminal().map_err(ReceiverError::Store)
}

/// Idempotent re-delivery of an attempt: return the existing receipt, if any,
/// without re-dispatching or re-accepting.
pub fn replay(store: &Store, handoff_id: &HandoffId) -> Result<Option<Receipt>, ReceiverError> {
    store
        .get_by_handoff_id(handoff_id)
        .map_err(ReceiverError::Store)
}

/// Human re-confirm of an uncertain (`handed-off`) receipt. As a no-op it
/// returns the existing receipt; an actual re-dispatch goes through
/// [`super::receive`] with the same handoff id (idempotent — never a double
/// accept).
pub fn reconfirm(store: &Store, handoff_id: &HandoffId) -> Result<Option<Receipt>, ReceiverError> {
    store
        .get_by_handoff_id(handoff_id)
        .map_err(ReceiverError::Store)
}

/// Status query against coordinator/planner. As a no-op it returns the existing
/// receipt; a real query records the external answer verbatim but the board's
/// own receipt (and the digest it verified) stays the final-state authority.
pub fn status_query(
    store: &Store,
    handoff_id: &HandoffId,
) -> Result<Option<Receipt>, ReceiverError> {
    store
        .get_by_handoff_id(handoff_id)
        .map_err(ReceiverError::Store)
}
