//! The draft store (G5): a card draft's `factory_kind` is fixed atomically at
//! creation and read-only everywhere else. The single permitted transition is
//! the explicit [`promote_to_factory_request`], which is monotonic and never
//! auto-fires.

use std::fmt;

use crate::digest::{compute, CanonicalRequest, Identity, SCHEMA_VERSION};
use crate::kind::FactoryKind;
use crate::outbox::{RequestRecord, Store, StoreError};

/// A board card draft. `factory_kind` is a first-class column from the instant
/// the row exists — there is no "kind unset" state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub draft_id: String,
    pub factory_kind: FactoryKind,
    pub title: String,
    pub body: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Set when the draft is published to GitHub (G4); `None` otherwise.
    pub published_at: Option<i64>,
}

/// Errors from the draft/card API.
#[derive(Debug)]
pub enum CardError {
    /// No draft with the given id exists.
    NotFound,
    /// The draft is already a factory request — the `ordinary → factory-request`
    /// transition is monotonic and one-way.
    AlreadyPromoted,
    /// An underlying outbox-store error.
    Store(StoreError),
}

impl fmt::Display for CardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CardError::NotFound => write!(f, "draft not found"),
            CardError::AlreadyPromoted => write!(f, "draft is already a factory request"),
            CardError::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for CardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CardError::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StoreError> for CardError {
    fn from(e: StoreError) -> Self {
        CardError::Store(e)
    }
}

/// Create a draft, persisting its `factory_kind` in the same INSERT as the row
/// (atomic — never a separate later update).
pub fn create_draft(
    store: &Store,
    factory_kind: FactoryKind,
    title: &str,
    body: &str,
) -> Result<Draft, CardError> {
    store
        .insert_draft(factory_kind, title, body)
        .map_err(CardError::Store)
}

/// Read a draft's `factory_kind` (preserved across reopen).
pub fn factory_kind_of(store: &Store, draft_id: &str) -> Result<FactoryKind, CardError> {
    store
        .get_draft_by_id(draft_id)
        .map_err(CardError::Store)?
        .map(|d| d.factory_kind)
        .ok_or(CardError::NotFound)
}

/// The explicit `ordinary → factory-request` transition: atomically promote the
/// draft's kind and build the corresponding request record. It is monotonic
/// (a second promote is refused) and never auto-fires a handoff.
///
/// The returned request carries the draft's `factory_kind` (the G5 invariant)
/// and the draft's body. `identity`/`revision`/`factory`/`actor` are resolved
/// by the board's link step at "Process with factory" time (out of scope here),
/// so this record has them empty; the caller fills them before handing off.
pub fn promote_to_factory_request(
    store: &Store,
    draft_id: &str,
) -> Result<RequestRecord, CardError> {
    let draft = store
        .get_draft_by_id(draft_id)
        .map_err(CardError::Store)?
        .ok_or(CardError::NotFound)?;

    if draft.factory_kind != FactoryKind::Ordinary {
        return Err(CardError::AlreadyPromoted);
    }

    let updated = store
        .promote_draft_kind(draft_id)
        .map_err(CardError::Store)?;
    if updated == 0 {
        // Lost a race: another writer promoted it first.
        return Err(CardError::AlreadyPromoted);
    }

    let canonical = CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: Identity::new("", "", 0),
        revision: String::new(),
        factory: String::new(),
        actor: String::new(),
        body: draft.body.clone(),
    };
    let digest = compute(&canonical);

    Ok(RequestRecord {
        schema_version: SCHEMA_VERSION,
        factory_kind: FactoryKind::FactoryRequest,
        digest,
        identity: String::new(),
        revision: String::new(),
        factory: String::new(),
        actor: String::new(),
        actor_source: String::new(),
        body: draft.body,
    })
}
