//! The handoff seam: what the receiver hands to coordinator/planner, and the
//! injectable transport that performs the one external call.

use crate::digest::CanonicalRequest;
use crate::outbox::HandoffId;

/// One explicit "Process with factory" handoff: the caller's attempt id plus
/// the canonical request to hand over.
#[derive(Debug, Clone)]
pub struct Handoff {
    pub handoff_id: HandoffId,
    pub request: CanonicalRequest,
}

/// The coordinator/planner response, recorded verbatim but treated as
/// **untrusted external data**: it never overrides the board's own committed
/// state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalResponse(pub String);

impl AsRef<str> for ExternalResponse {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The result of the external handoff call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffResult {
    pub accepted: bool,
    pub response: ExternalResponse,
}

/// The injectable seam for the single external call. The real
/// coordinator/planner transport (subprocess/CLI) is a later slice; the
/// receiver and outbox are fully testable against a fake implementation now.
pub trait HandoffTransport {
    fn handoff(&self, handoff: &Handoff) -> HandoffResult;
}
