//! Create reconciliation: surface `issued` intents for manual resolution and
//! pre-populate candidates. Read-only — it never auto-issues, auto-links, or
//! auto-re-plays.

use crate::create::{marker_comment, Candidate, CreateError, GitHubClient, RepoIdentity};
use crate::outbox::{CreateIntent, Store, StoreError};

/// The `issued` intents (outcome unknown) — the "awaiting link" population.
pub fn startup_sweep(store: &Store) -> Result<Vec<CreateIntent>, CreateError> {
    store.list_issued().map_err(CreateError::Store)
}

/// Read-only candidate pre-population: for each `issued` intent, run the
/// primary marker search. Returns each intent with its marker-search
/// candidates. Never links and never re-issues.
pub fn search_marker(
    store: &Store,
    client: &impl GitHubClient,
) -> Result<Vec<(CreateIntent, Vec<Candidate>)>, CreateError> {
    let intents = store.list_issued().map_err(CreateError::Store)?;
    let mut out = Vec::with_capacity(intents.len());
    for intent in intents {
        let repo = RepoIdentity::parse(&intent.repo).ok_or_else(|| {
            CreateError::Store(StoreError::InvalidData(
                "create intent repo is not canonical owner/repo".to_string(),
            ))
        })?;
        let query = marker_comment(&intent.marker);
        let found = client.search_issues(&repo, &query);
        out.push((intent, found));
    }
    Ok(out)
}
