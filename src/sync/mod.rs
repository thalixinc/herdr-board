//! GitHub issue pull sync (VS1): fetch issues, mirror them onto the board as
//! ordinary cards, and surface GitHub-side drift as a visible conflict — never
//! silently overwriting board state.

mod client;
mod conflict;

pub use client::{Credentials, IssueFull, PullClient, RealGitHubClient};
pub use conflict::field_diffs;

use std::fmt;

use crate::create::RepoIdentity;
use crate::digest::Identity;
use crate::kind::FactoryKind;
use crate::outbox::{CanonicalFields, Card, Conflict, Store, StoreError};

/// The default board column a newly pulled card lands in.
pub const DEFAULT_COLUMN: &str = "to-do";

/// A count of what one sync pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncSummary {
    /// New cards inserted.
    pub inserted: usize,
    /// Existing cards whose revision was unchanged (no content change).
    pub unchanged: usize,
    /// Existing cards whose revision changed (flagged `apply-pending`).
    pub conflicted: usize,
}

/// A sync/apply/defer failure.
#[derive(Debug)]
pub enum SyncError {
    /// The outbox store failed.
    Store(StoreError),
    /// `apply_changes` could not fetch the requested issue.
    IssueNotFound { number: u64 },
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Store(e) => write!(f, "store error: {e}"),
            SyncError::IssueNotFound { number } => write!(f, "issue #{number} not found"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SyncError::Store(e) => Some(e),
            SyncError::IssueNotFound { .. } => None,
        }
    }
}

/// Map a fetched issue to its canonical fields and its revision token
/// (GitHub `updated_at`, verbatim).
pub fn map(issue: IssueFull) -> (CanonicalFields, String) {
    let fields = CanonicalFields {
        title: issue.title,
        body: issue.body,
        state: issue.state,
        state_reason: issue.state_reason,
        labels: issue.labels,
        assignee: issue.assignee,
        milestone: issue.milestone,
    };
    (fields, issue.updated_at)
}

/// One explicit fetch-to-completion: mirror every fetched issue onto the board.
///
/// For each issue: no card → insert (default column, ordinary, no conflict);
/// unchanged revision → `touch_card`; changed revision → compute the drifted
/// fields, `record_conflict`, and flag `apply-pending` (never silently
/// overwrite).
pub fn sync(
    store: &Store,
    client: &impl PullClient,
    repo: &RepoIdentity,
) -> Result<SyncSummary, SyncError> {
    let now = now_unix();
    let mut summary = SyncSummary::default();

    for issue in client.list_issues(repo) {
        let number = issue.number;
        let url = issue.url.clone();
        let (fields, revision) = map(issue);
        let identity = Identity::new(repo.owner.as_str(), repo.repo.as_str(), number);

        match store.get_card(&identity).map_err(SyncError::Store)? {
            None => {
                let card = Card {
                    identity,
                    url,
                    fields,
                    column: DEFAULT_COLUMN.to_owned(),
                    factory_kind: FactoryKind::Ordinary,
                    revision,
                    conflict: Conflict::None,
                    synced_at: now,
                };
                store.insert_card(&card).map_err(SyncError::Store)?;
                summary.inserted += 1;
            }
            Some(card) => {
                if card.revision == revision {
                    store.touch_card(&identity, now).map_err(SyncError::Store)?;
                    summary.unchanged += 1;
                } else {
                    let diffs = field_diffs(&card.fields, &fields);
                    store
                        .record_conflict(&identity, &diffs, now)
                        .map_err(SyncError::Store)?;
                    summary.conflicted += 1;
                }
            }
        }
    }

    Ok(summary)
}

/// Accept the fetched canonical fields for one card: `apply_conflict` with the
/// seven shared fields plus the new revision, clearing the conflict. Board-local
/// `column`/`factory_kind` are untouched.
pub fn apply_changes(
    store: &Store,
    client: &impl PullClient,
    repo: &RepoIdentity,
    number: u64,
) -> Result<Card, SyncError> {
    let issue = client
        .fetch_issue(repo, number)
        .ok_or(SyncError::IssueNotFound { number })?;
    let (fields, revision) = map(issue);
    let identity = Identity::new(repo.owner.as_str(), repo.repo.as_str(), number);
    store
        .apply_conflict(&identity, &fields, &revision)
        .map_err(SyncError::Store)
}

/// Defer a conflict: keep the `apply-pending` flag (it re-surfaces next sync).
pub fn defer_changes(store: &Store, identity: &Identity) -> Result<(), SyncError> {
    store.defer_conflict(identity).map_err(SyncError::Store)
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
