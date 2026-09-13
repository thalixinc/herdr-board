//! The explicit "Process with factory" entrypoint: promote the draft, resolve
//! the linked card, prove the actor, build the full six-field
//! [`CanonicalRequest`], and hand it to the G3 receiver.

use std::fmt;

use crate::card::{promote_to_factory_request, CardError};
use crate::digest::{CanonicalRequest, Identity};
use crate::kind::FactoryKind;
use crate::outbox::{HandoffId, Receipt, Store, StoreError};
use crate::receiver::{receive, Handoff, HandoffTransport, ReceiverError, TrustRoot};

/// Errors from [`process_with_factory`].
#[derive(Debug)]
pub enum ProcessError {
    /// The draft could not be promoted (e.g. already a factory request).
    Card(CardError),
    /// The linked card does not exist.
    CardNotFound,
    /// The receiver refused the handoff or the transport failed.
    Receiver(ReceiverError),
    /// An outbox-store error.
    Store(StoreError),
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessError::Card(e) => write!(f, "promote failed: {e}"),
            ProcessError::CardNotFound => write!(f, "linked card not found"),
            ProcessError::Receiver(e) => write!(f, "receiver error: {e}"),
            ProcessError::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for ProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProcessError::Card(e) => Some(e),
            ProcessError::Receiver(e) => Some(e),
            ProcessError::Store(e) => Some(e),
            ProcessError::CardNotFound => None,
        }
    }
}

impl From<CardError> for ProcessError {
    fn from(e: CardError) -> Self {
        ProcessError::Card(e)
    }
}

impl From<ReceiverError> for ProcessError {
    fn from(e: ReceiverError) -> Self {
        ProcessError::Receiver(e)
    }
}

impl From<StoreError> for ProcessError {
    fn from(e: StoreError) -> Self {
        ProcessError::Store(e)
    }
}

/// The explicit "Process with factory" action: promote the draft, resolve the
/// linked card, prove the actor, build the full six-field [`CanonicalRequest`],
/// and hand it to the receiver.
///
/// The placeholder `RequestRecord`/digest that [`promote_to_factory_request`]
/// returns is **discarded** — `receive()` derives the real digest from the
/// fully-populated request (draft body + card identity/revision + target
/// factory + trust-root actor).
pub fn process_with_factory(
    store: &Store,
    transport: &impl HandoffTransport,
    draft_id: &str,
    card_identity: &Identity,
    target_factory: &str,
) -> Result<Receipt, ProcessError> {
    // 1. Promote the draft (atomic `ordinary → factory-request`). The returned
    //    record's body is the draft's board-authored payload; its digest is a
    //    placeholder over an empty identity and is discarded here.
    let promoted = promote_to_factory_request(store, draft_id).map_err(ProcessError::Card)?;
    let body = promoted.body;

    // 2. Resolve the linked card for its identity + revision.
    let card = store
        .get_card(card_identity)
        .map_err(ProcessError::Store)?
        .ok_or(ProcessError::CardNotFound)?;

    // 3. Prove the actor from the trust root — never the card or the body.
    let actor = TrustRoot::from_env().prove();

    // 4. Build the full six-field request.
    let request = CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: card.identity,
        revision: card.revision,
        factory: target_factory.to_owned(),
        actor: actor.value,
        body,
    };

    // 5. Hand off through the receiver (proves, verifies, dedups, dispatches).
    let handoff = Handoff {
        handoff_id: HandoffId::new_v4(),
        request,
    };
    receive(store, &handoff, transport).map_err(ProcessError::Receiver)
}
