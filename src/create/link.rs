//! Manual link / cancel entrypoints and candidate search. Every action here is
//! human-gated: link adopts an existing issue, cancel only stops tracking, and
//! candidates are suggestions — never auto-linked, never auto-re-issued.

use crate::create::{marker_comment, Candidate, CreateError, GitHubClient, RepoIdentity};
use crate::outbox::{CreateIntent, CreateOutcome, IntentId, Store, StoreError};
use crate::receiver::STALENESS_THRESHOLD;

/// Link an `issued` intent to a known issue: verify the issue exists, then
/// adopt it (`mark_created`) — the board never re-creates.
pub fn link(
    store: &Store,
    client: &impl GitHubClient,
    intent_id: &IntentId,
    issue_number: u64,
) -> Result<CreateOutcome, CreateError> {
    let intent = find_issued(store, intent_id)?;
    let repo = RepoIdentity::parse(&intent.repo).ok_or_else(|| {
        CreateError::Store(StoreError::InvalidData(
            "create intent repo is not canonical owner/repo".to_string(),
        ))
    })?;

    // Never auto-link a non-existent issue.
    if client.get_issue(&repo, issue_number).is_none() {
        return Err(CreateError::NotFound);
    }

    store
        .mark_created(intent_id, issue_number)
        .map_err(CreateError::Store)?;
    Ok(CreateOutcome::Created)
}

/// Cancel a `pending`/`issued` intent. Cancel only stops tracking — it never
/// deletes anything on GitHub.
pub fn cancel(store: &Store, intent_id: &IntentId) -> Result<CreateOutcome, CreateError> {
    store
        .mark_cancelled(intent_id)
        .map_err(CreateError::Store)?;
    Ok(CreateOutcome::Cancelled)
}

/// Candidate matches for an `issued` intent, marker-first, then exact-title
/// fallback scoped to the repo and recent issues. Suggestions only.
pub fn candidates(
    store: &Store,
    client: &impl GitHubClient,
    intent_id: &IntentId,
) -> Result<Vec<Candidate>, CreateError> {
    let intent = find_issued(store, intent_id)?;
    let repo = RepoIdentity::parse(&intent.repo).ok_or_else(|| {
        CreateError::Store(StoreError::InvalidData(
            "create intent repo is not canonical owner/repo".to_string(),
        ))
    })?;

    // Primary: exact idempotency-marker match.
    let marker_query = marker_comment(&intent.marker);
    let by_marker = client.search_issues(&repo, &marker_query);
    if !by_marker.is_empty() {
        return Ok(by_marker);
    }

    // Secondary: exact-title search, filtered to recent issues only.
    let title_query = normalize_title(&intent.title);
    let by_title = client.search_issues(&repo, &title_query);
    let threshold = STALENESS_THRESHOLD.as_secs() as i64;
    let issued_at = intent.issued_at.unwrap_or(intent.created_at);
    Ok(by_title
        .into_iter()
        .filter(|c| {
            c.created_at
                .is_some_and(|ts| (ts - issued_at).abs() <= threshold)
        })
        .collect())
}

/// Trim, collapse internal whitespace to single spaces, and ASCII-lowercase.
pub(crate) fn normalize_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn find_issued(store: &Store, intent_id: &IntentId) -> Result<CreateIntent, CreateError> {
    store
        .list_issued()
        .map_err(CreateError::Store)?
        .into_iter()
        .find(|i| i.intent_id == *intent_id)
        .ok_or(CreateError::NotFound)
}
