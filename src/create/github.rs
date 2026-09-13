//! The GitHub client seam: an injectable trait (no real network, no
//! credentials in any code path) plus the canonical repo identity, the
//! three-way create result, and the reconciliation marker.

use crate::outbox::{CreateIntent, Marker};

/// The canonical target repository: `owner/repo`, both ASCII-lowercased.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoIdentity {
    pub owner: String,
    pub repo: String,
}

impl RepoIdentity {
    /// Construct a canonical identity, lowercasing both parts.
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> RepoIdentity {
        RepoIdentity {
            owner: owner.into().to_lowercase(),
            repo: repo.into().to_lowercase(),
        }
    }

    /// The canonical `owner/repo` form.
    pub fn canonical(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Parse a canonical `owner/repo` string, lowercasing both parts.
    pub fn parse(s: &str) -> Option<RepoIdentity> {
        let (owner, repo) = s.split_once('/')?;
        Some(RepoIdentity::new(owner, repo))
    }
}

impl std::fmt::Display for RepoIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// The three-way result of a create call.
///
/// `Uncertain` is the irreducible transport case: the request may or may not
/// have succeeded (timeout, drop, crash). It is **never** safe to re-issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateResult {
    /// GitHub returned the new issue number.
    Created(u64),
    /// GitHub returned a definite error — nothing was created.
    Failed(String),
    /// The response was lost — outcome unknown, must be resolved manually.
    Uncertain(String),
}

/// The board's push patch: which GitHub-canonical fields to change on an
/// existing issue.
///
/// `title`/`body`/`labels` are single-`Option` (replace-on-set; `labels`
/// replaces the whole set). `assignee`/`milestone` are double-`Option`:
/// `None` leaves the field unchanged, `Some(None)` clears it, and
/// `Some(Some(v))` sets it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IssuePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub labels: Option<Vec<String>>,
    pub assignee: Option<Option<String>>,
    pub milestone: Option<Option<String>>,
}

/// The three-way result of an update (PATCH) call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateResult {
    /// GitHub applied the patch; `updated_at` is the new revision token.
    Updated { updated_at: String },
    /// GitHub returned a definite error — nothing was changed.
    Failed(String),
    /// The response was lost — outcome unknown, must be resolved manually.
    Uncertain(String),
}

/// A candidate issue for manual linking (a suggestion, never an auto-decision).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub number: u64,
    pub title: String,
    pub body_snippet: String,
    /// Issue creation time; used to scope the title-fallback to recent issues.
    pub created_at: Option<i64>,
}

/// A known issue, as returned by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
}

/// The idempotency-marker comment embedded in a created issue's body (an HTML
/// comment: invisible in rendered view, present in the raw body for search).
///
/// The marker is a public opaque UUID — **not** a credential — so embedding it
/// leaks nothing.
pub fn marker_comment(marker: &Marker) -> String {
    format!("<!-- herdr-board:intent:{marker} -->")
}

/// The injectable seam for the board→GitHub publish path. The real HTTP client
/// is a later slice; `issue`/`link`/`candidates` are testable against a fake.
pub trait GitHubClient {
    /// Create an issue. `marker_comment` is appended to `intent.body` only at
    /// send time — the stored body stays verbatim.
    fn create_issue(
        &self,
        repo: &RepoIdentity,
        intent: &CreateIntent,
        marker_comment: &str,
    ) -> CreateResult;

    /// Search the target repo. Marker queries look for the HTML comment;
    /// title queries look for the normalized exact title.
    fn search_issues(&self, repo: &RepoIdentity, query: &str) -> Vec<Candidate>;

    /// Look up a known issue by number (existence check for manual linking).
    fn get_issue(&self, repo: &RepoIdentity, number: u64) -> Option<Issue>;

    /// Patch an existing issue. Only the fields present in `patch` are sent;
    /// on success the result carries the response's new `updated_at`.
    fn update_issue(&self, repo: &RepoIdentity, number: u64, patch: &IssuePatch) -> UpdateResult;
}
