//! The real board→cf-queue transport: the single translation point from the
//! board's canonical request to cf-queue's field contract, plus the three-way
//! outcome mapping. The concrete wire invocation is a later integration; VS3
//! delivers the translation and the injectable seam, fake-tested.

use crate::digest::CanonicalRequest;
use crate::sync::Credentials;

use super::{ExternalResponse, HandoffResult, HandoffTransport};

/// The board→cf-queue field contract: the five fields the board hands to the
/// coordinator/planner's queue. This aligns on the *field contract* only — it
/// never re-owns cf-queue's request encoding (the concrete wire form belongs
/// to cf-queue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfQueueContract {
    pub identity: String,
    pub revision: String,
    pub factory: String,
    pub actor: String,
    pub body: String,
}

impl CfQueueContract {
    /// Build the contract fields from a canonical request. `factory_kind` is a
    /// board-local discriminator and is not part of the contract; `identity`
    /// is canonicalized, the other four fields are verbatim (never re-encoded).
    pub fn from_request(request: &CanonicalRequest) -> CfQueueContract {
        CfQueueContract {
            identity: request.identity.canonical(),
            revision: request.revision.clone(),
            factory: request.factory.clone(),
            actor: request.actor.clone(),
            body: request.body.clone(),
        }
    }
}

/// The external outcome of a cf-queue submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfSubmission {
    /// The queue accepted the request.
    Accepted { response: String },
    /// The queue refused the request.
    Refused { response: String },
    /// The queue was unreachable or timed out.
    Failed { reason: String },
}

/// The real board→cf-queue transport. Holds credentials in memory only (never
/// persisted, never logged). The `send` closure is the injected seam where the
/// concrete wire invocation will land; the transport itself owns the
/// translation and the three-way outcome mapping.
pub struct RealHandoffTransport<F> {
    _credentials: Option<Credentials>,
    send: F,
}

impl<F: Fn(&CfQueueContract) -> CfSubmission> RealHandoffTransport<F> {
    /// Build the transport from in-memory credentials and a submission closure.
    pub fn new(credentials: Option<Credentials>, send: F) -> Self {
        Self {
            _credentials: credentials,
            send,
        }
    }
}

impl<F: Fn(&CfQueueContract) -> CfSubmission> HandoffTransport for RealHandoffTransport<F> {
    fn handoff(&self, request: &CanonicalRequest) -> HandoffResult {
        let contract = CfQueueContract::from_request(request);
        match (self.send)(&contract) {
            CfSubmission::Accepted { response } => {
                HandoffResult::Accepted(ExternalResponse::new(response))
            }
            CfSubmission::Refused { response } => {
                HandoffResult::Refused(ExternalResponse::new(response))
            }
            CfSubmission::Failed { reason } => HandoffResult::Failed(reason),
        }
    }
}
