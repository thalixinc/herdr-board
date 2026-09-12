//! The publisher: the sealed board→GitHub create seam. Its single entrypoint
//! is [`issue`]; the write-ahead is insert `pending` → commit `issued` →
//! create → finalize, so a lost response parks the intent in `issued` (outcome
//! unknown) rather than silently re-issuing.

use std::fmt;

use crate::outbox::{CreateIntent, CreateOutcome, Store, StoreError};

mod github;
mod link;
mod reconcile;

pub use github::{marker_comment, Candidate, CreateResult, GitHubClient, Issue, RepoIdentity};
pub use link::{cancel, candidates, link};
pub use reconcile::{search_marker, startup_sweep};

/// The sealed publisher seam. Its single entrypoint is [`issue`]; there is no
/// path from "publish card" to a GitHub create that bypasses it, and no
/// background loop reaches it.
pub struct Publisher;

/// Errors the create path can return.
#[derive(Debug)]
pub enum CreateError {
    /// The create may or may not have landed; the intent stays `issued`.
    Uncertain(String),
    /// GitHub returned a definite error; the intent is `failed` (terminal).
    Failed(String),
    /// An underlying outbox-store error.
    Store(StoreError),
    /// The intent, or the issue being linked, does not exist.
    NotFound,
}

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CreateError::Uncertain(e) => write!(f, "create outcome unknown: {e}"),
            CreateError::Failed(e) => write!(f, "create failed: {e}"),
            CreateError::Store(e) => write!(f, "store error: {e}"),
            CreateError::NotFound => write!(f, "intent or issue not found"),
        }
    }
}

impl std::error::Error for CreateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CreateError::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StoreError> for CreateError {
    fn from(e: StoreError) -> Self {
        CreateError::Store(e)
    }
}

/// Persist the intent, issue the create, and finalize the outcome.
///
/// Write-ahead: `insert_intent` (pending) → `mark_issued` (committed before the
/// external call) → `client.create_issue` → `mark_created`/`mark_failed`, or
/// leave `issued` on `Uncertain`.
pub fn issue(
    store: &Store,
    client: &impl GitHubClient,
    intent: &CreateIntent,
) -> Result<CreateOutcome, CreateError> {
    let repo = RepoIdentity::parse(&intent.repo).ok_or_else(|| {
        CreateError::Store(StoreError::InvalidData(
            "create intent repo is not canonical owner/repo".to_string(),
        ))
    })?;

    // 1. Persist the intent in `pending` (unique marker dedups a re-insert).
    store.insert_intent(intent).map_err(CreateError::Store)?;
    // 2. Commit `issued` before the external call — the block-auto-replay guard.
    store
        .mark_issued(&intent.intent_id)
        .map_err(CreateError::Store)?;

    // 3. The one external create. The marker comment is appended at send only.
    let comment = marker_comment(&intent.marker);
    match client.create_issue(&repo, intent, &comment) {
        CreateResult::Created(number) => {
            store
                .mark_created(&intent.intent_id, number)
                .map_err(CreateError::Store)?;
            Ok(CreateOutcome::Created)
        }
        CreateResult::Failed(reason) => {
            store
                .mark_failed(&intent.intent_id, &reason)
                .map_err(CreateError::Store)?;
            Err(CreateError::Failed(reason))
        }
        CreateResult::Uncertain(reason) => {
            // Leave `issued` — outcome unknown, resolved only by a human.
            Err(CreateError::Uncertain(reason))
        }
    }
}
