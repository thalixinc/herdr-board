//! Card→issue push (VS2): draft publishing plus the pending-write push
//! orchestration guarded by the base-revision gate — never silently
//! overwriting a concurrent GitHub edit.

mod conflict;

pub use conflict::field_diffs;

use std::fmt;

use crate::create::{
    issue as create_issue, CreateError, GitHubClient, IssuePatch, RepoIdentity, UpdateResult,
};
use crate::digest::Identity;
use crate::outbox::{
    CanonicalFields, Card, CardFieldDiff, Conflict, CreateIntent, CreateOutcome, Marker,
    PendingWrite, Store, StoreError, WriteOutcome,
};
use crate::sync::{PullClient, SyncError, DEFAULT_COLUMN};
use crate::Draft;

/// A publish/push failure.
#[derive(Debug)]
pub enum PushError {
    /// The outbox store failed.
    Store(StoreError),
    /// The card→issue create failed.
    Create(CreateError),
    /// The post-discard re-sync failed.
    Sync(SyncError),
    /// No draft with the given id.
    DraftNotFound,
    /// The issue to push to does not exist (fetch returned `None`).
    IssueNotFound { number: u64 },
    /// No card exists for this identity.
    CardNotFound,
    /// The card has a VS1 pull conflict (`apply-pending`); resolve it first.
    PullConflictPending,
    /// No unresolved pending write exists for this card.
    NoPendingWrite,
    /// GitHub changed since the edit; resolve via `apply_push`/`discard_push`.
    Conflict { diffs: Vec<CardFieldDiff> },
    /// GitHub returned a definite error; the write is marked `failed`.
    UpdateFailed(String),
    /// The update response was lost; the write is marked `uncertain`.
    UpdateUncertain(String),
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PushError::Store(e) => write!(f, "store error: {e}"),
            PushError::Create(e) => write!(f, "create error: {e}"),
            PushError::Sync(e) => write!(f, "sync error: {e}"),
            PushError::DraftNotFound => f.write_str("draft not found"),
            PushError::IssueNotFound { number } => write!(f, "issue #{number} not found"),
            PushError::CardNotFound => f.write_str("card not found"),
            PushError::PullConflictPending => {
                f.write_str("card has an unresolved pull conflict; apply or defer it first")
            }
            PushError::NoPendingWrite => f.write_str("no pending write for this card"),
            PushError::Conflict { diffs } => {
                write!(f, "{} field(s) drifted on GitHub", diffs.len())
            }
            PushError::UpdateFailed(reason) => write!(f, "update failed: {reason}"),
            PushError::UpdateUncertain(reason) => write!(f, "update outcome unknown: {reason}"),
        }
    }
}

impl std::error::Error for PushError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PushError::Store(e) => Some(e),
            PushError::Create(e) => Some(e),
            PushError::Sync(e) => Some(e),
            _ => None,
        }
    }
}

/// Publish a draft to GitHub: build a `CreateIntent` from the draft's
/// title/body/factory_kind, issue the create, and link a card seeded from a
/// re-fetch of the created issue.
pub fn publish_draft<C>(
    store: &Store,
    client: &C,
    draft: &Draft,
    repo: &RepoIdentity,
    labels: &[String],
    assignee: Option<&str>,
) -> Result<Card, PushError>
where
    C: PullClient + GitHubClient,
{
    let labels_json = serde_json::to_string(labels)
        .map_err(|e| PushError::Store(StoreError::InvalidData(e.to_string())))?;
    let mut intent = CreateIntent::new(
        Marker::new_v4(),
        repo.canonical(),
        draft.title.clone(),
        draft.body.clone(),
        labels_json,
        draft.factory_kind,
    );
    intent.assignee = assignee.map(str::to_owned);

    match create_issue(store, client, &intent) {
        Ok(CreateOutcome::Created) => {}
        Ok(other) => {
            return Err(PushError::Create(CreateError::Store(
                StoreError::InvalidData(format!(
                    "create returned unexpected outcome {}",
                    other.as_str()
                )),
            )))
        }
        Err(e) => return Err(PushError::Create(e)),
    }

    let created = store
        .get_by_marker(&intent.marker)
        .map_err(PushError::Store)?
        .ok_or_else(|| PushError::Store(StoreError::NotFound))?;
    let number = created.issue_number.ok_or_else(|| {
        PushError::Store(StoreError::InvalidData(
            "created intent lacks issue number".to_owned(),
        ))
    })?;

    let identity = Identity::new(repo.owner.as_str(), repo.repo.as_str(), number);
    let card = match client.fetch_issue(repo, number) {
        Some(issue) => {
            let url = issue.url.clone();
            let (fields, revision) = crate::sync::map(issue);
            Card {
                identity: identity.clone(),
                url,
                fields,
                column: DEFAULT_COLUMN.to_owned(),
                factory_kind: draft.factory_kind,
                revision,
                conflict: Conflict::None,
                synced_at: now_unix(),
            }
        }
        None => Card {
            identity: identity.clone(),
            url: format!(
                "https://github.com/{}/{}/issues/{number}",
                repo.owner, repo.repo
            ),
            fields: CanonicalFields {
                title: draft.title.clone(),
                body: draft.body.clone(),
                state: "open".to_owned(),
                state_reason: None,
                labels: labels.to_vec(),
                assignee: assignee.map(str::to_owned),
                milestone: None,
            },
            column: DEFAULT_COLUMN.to_owned(),
            factory_kind: draft.factory_kind,
            revision: String::new(),
            conflict: Conflict::None,
            synced_at: now_unix(),
        },
    };

    store.insert_card(&card).map_err(PushError::Store)?;
    store
        .get_card(&identity)
        .map_err(PushError::Store)?
        .ok_or_else(|| PushError::Store(StoreError::NotFound))
}

/// Push a card's pending write to GitHub, guarded by the base revision.
///
/// Refuses while the card is `apply-pending`; fetches the issue and compares
/// its `updated_at` against the write's pinned `base_revision`. Equal → PATCH
/// and finalize `written`. Different → surface a field-level conflict (never
/// patch).
pub fn push_changes<C>(store: &Store, client: &C, identity: &Identity) -> Result<Card, PushError>
where
    C: PullClient + GitHubClient,
{
    let card = store
        .get_card(identity)
        .map_err(PushError::Store)?
        .ok_or(PushError::CardNotFound)?;
    if card.conflict == Conflict::ApplyPending {
        return Err(PushError::PullConflictPending);
    }

    let write = store
        .get_pending_write(identity)
        .map_err(PushError::Store)?
        .ok_or(PushError::NoPendingWrite)?;

    let repo = RepoIdentity::new(identity.owner.as_str(), identity.repo.as_str());
    let number = identity.number;
    let issue = client
        .fetch_issue(&repo, number)
        .ok_or(PushError::IssueNotFound { number })?;

    if issue.updated_at != write.base_revision {
        return Err(PushError::Conflict {
            diffs: conflict::field_diffs(&write, &issue),
        });
    }

    push_patch(store, client, identity, &repo, number, &write)
}

/// Human-confirmed board-wins PATCH: apply the pending write over GitHub
/// without re-running the gate, then finalize `written`.
pub fn apply_push<C>(store: &Store, client: &C, identity: &Identity) -> Result<Card, PushError>
where
    C: GitHubClient,
{
    let write = store
        .get_pending_write(identity)
        .map_err(PushError::Store)?
        .ok_or(PushError::NoPendingWrite)?;
    let repo = RepoIdentity::new(identity.owner.as_str(), identity.repo.as_str());
    push_patch(store, client, identity, &repo, identity.number, &write)
}

/// Discard the pending write and re-sync the card from GitHub.
pub fn discard_push<C>(store: &Store, client: &C, identity: &Identity) -> Result<(), PushError>
where
    C: PullClient,
{
    store.discard_write(identity).map_err(PushError::Store)?;
    let repo = RepoIdentity::new(identity.owner.as_str(), identity.repo.as_str());
    crate::sync::sync(store, client, &repo).map_err(PushError::Sync)?;
    Ok(())
}

fn push_patch<C: GitHubClient>(
    store: &Store,
    client: &C,
    identity: &Identity,
    repo: &RepoIdentity,
    number: u64,
    write: &PendingWrite,
) -> Result<Card, PushError> {
    let patch = IssuePatch {
        title: write.title.clone(),
        body: write.body.clone(),
        labels: write.labels.clone(),
        assignee: write.assignee.clone(),
        milestone: write.milestone.clone(),
    };
    match client.update_issue(repo, number, &patch) {
        UpdateResult::Updated { updated_at } => {
            store
                .finalize_write(identity, WriteOutcome::Written, Some(&updated_at))
                .map_err(PushError::Store)?;
            store
                .get_card(identity)
                .map_err(PushError::Store)?
                .ok_or(PushError::CardNotFound)
        }
        UpdateResult::Failed(reason) => {
            store
                .finalize_write(identity, WriteOutcome::Failed, None)
                .map_err(PushError::Store)?;
            Err(PushError::UpdateFailed(reason))
        }
        UpdateResult::Uncertain(reason) => {
            store
                .finalize_write(identity, WriteOutcome::Uncertain, None)
                .map_err(PushError::Store)?;
            Err(PushError::UpdateUncertain(reason))
        }
    }
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
