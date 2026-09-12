//! The handoff seam: the injectable transport to the external coordinator/planner.

use crate::digest::CanonicalRequest;
use crate::outbox::HandoffId;

/// One handoff: the caller's id for this attempt, plus the request to dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub handoff_id: HandoffId,
    pub request: CanonicalRequest,
}

/// The external coordinator/planner's response, recorded verbatim and treated
/// as UNTRUSTED. It never overrides the board's own state or the digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalResponse {
    text: String,
}

impl ExternalResponse {
    /// Build a verbatim external response.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The raw response, verbatim.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// The external system's decision for one handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffResult {
    /// Accepted by the coordinator/planner.
    Accepted(ExternalResponse),
    /// Refused by the coordinator/planner (a normal decision, not a failure).
    Refused(ExternalResponse),
    /// The transport could not deliver (unreachable, timeout, …). Outcome is
    /// unknown: the receipt stays `handed-off` and the receiver surfaces a
    /// [`super::ReceiverError::Transport`].
    Failed(String),
}

/// The injectable handoff seam. The real coordinator/planner transport is a
/// later slice; tests and the demo inject a fake.
///
/// Async-free by contract: the receiver calls [`HandoffTransport::handoff`]
/// synchronously between two committed write-ahead transactions.
pub trait HandoffTransport {
    /// Perform the one external handoff and return its decision.
    fn handoff(&self, request: &CanonicalRequest) -> HandoffResult;
}
