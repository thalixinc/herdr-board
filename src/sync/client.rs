//! The read-only GitHub pull client: the injectable seam for fetching issues,
//! plus the real `ureq` implementation.

use std::path::PathBuf;

use serde::Deserialize;

use crate::create::RepoIdentity;

/// A fully-expanded GitHub issue as fetched from the read API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueFull {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub milestone: Option<String>,
    /// GitHub `updated_at`, verbatim — the revision gate for conflict detection.
    pub updated_at: String,
    pub url: String,
}

/// The read-only pull seam: fetch the issues to mirror onto the board.
///
/// Infallible by contract: a real client returns what it can and treats an
/// unreachable API as an empty list / `None` — the board never blocks on the
/// network for sync; it reflects the last successful fetch.
pub trait PullClient {
    /// Every issue in the repo (single page, hard-capped).
    fn list_issues(&self, repo: &RepoIdentity) -> Vec<IssueFull>;

    /// One issue by number, if it exists and is reachable.
    fn fetch_issue(&self, repo: &RepoIdentity, number: u64) -> Option<IssueFull>;
}

/// GitHub credentials, resolved from the environment. Held in memory only —
/// never written to any table, body, or prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    token: String,
}

impl Credentials {
    /// Resolve from the environment, mirroring `TrustRoot::from_env()`:
    /// (1) `HERDR_GITHUB_TOKEN`, else (2) `$HERDR_PLUGIN_CONFIG_DIR/github.token`.
    ///
    /// `None` means no credential is configured — the client runs
    /// unauthenticated.
    pub fn from_env() -> Option<Credentials> {
        if let Some(token) = non_empty_env("HERDR_GITHUB_TOKEN") {
            return Some(Credentials { token });
        }
        if let Some(config_dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
            let path = PathBuf::from(config_dir).join("github.token");
            if let Ok(token) = std::fs::read_to_string(path) {
                let token = token.trim().to_owned();
                if !token.is_empty() {
                    return Some(Credentials { token });
                }
            }
        }
        None
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// The real GitHub client (blocking `ureq`). Holds the token in memory only —
/// never persisted, never logged, never written to any table/body.
pub struct RealGitHubClient {
    token: Option<String>,
    api_base: String,
}

impl RealGitHubClient {
    /// Build a client; `None` runs unauthenticated (public repos, low limits).
    pub fn new(credentials: Option<Credentials>) -> RealGitHubClient {
        RealGitHubClient {
            token: credentials.map(|c| c.token),
            api_base: "https://api.github.com".to_owned(),
        }
    }

    fn get_text(&self, path: &str) -> Option<String> {
        let url = format!("{}{}", self.api_base, path);
        let mut req = ureq::get(&url)
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "herdr-board")
            .set("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
        req.call().ok()?.into_string().ok()
    }
}

impl PullClient for RealGitHubClient {
    fn list_issues(&self, repo: &RepoIdentity) -> Vec<IssueFull> {
        let path = format!(
            "/repos/{}/{}/issues?state=all&per_page=100",
            repo.owner, repo.repo
        );
        let text = match self.get_text(&path) {
            Some(text) => text,
            None => return Vec::new(),
        };
        serde_json::from_str::<Vec<GhIssue>>(&text)
            .map(|issues| issues.into_iter().map(IssueFull::from).collect())
            .unwrap_or_default()
    }

    fn fetch_issue(&self, repo: &RepoIdentity, number: u64) -> Option<IssueFull> {
        let path = format!("/repos/{}/{}/issues/{number}", repo.owner, repo.repo);
        let text = self.get_text(&path)?;
        serde_json::from_str::<GhIssue>(&text)
            .ok()
            .map(IssueFull::from)
    }
}

/// The wire shape of a GitHub REST issue (the subset the board mirrors).
#[derive(Debug, Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    state_reason: Option<String>,
    #[serde(default)]
    labels: Vec<GhLabel>,
    assignee: Option<GhUser>,
    milestone: Option<GhMilestone>,
    updated_at: String,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhMilestone {
    title: String,
}

impl From<GhIssue> for IssueFull {
    fn from(issue: GhIssue) -> IssueFull {
        IssueFull {
            number: issue.number,
            title: issue.title,
            body: issue.body.unwrap_or_default(),
            state: issue.state,
            state_reason: issue.state_reason,
            labels: issue.labels.into_iter().map(|label| label.name).collect(),
            assignee: issue.assignee.map(|user| user.login),
            milestone: issue.milestone.map(|milestone| milestone.title),
            updated_at: issue.updated_at,
            url: issue.html_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_issue_maps_to_issue_full() {
        let gh = GhIssue {
            number: 7,
            title: "Fix".to_owned(),
            body: Some("body".to_owned()),
            state: "open".to_owned(),
            state_reason: Some("reopened".to_owned()),
            labels: vec![GhLabel {
                name: "bug".to_owned(),
            }],
            assignee: Some(GhUser {
                login: "alice".to_owned(),
            }),
            milestone: Some(GhMilestone {
                title: "v1".to_owned(),
            }),
            updated_at: "2026-09-12T00:00:00Z".to_owned(),
            html_url: "https://github.com/o/r/issues/7".to_owned(),
        };

        let full = IssueFull::from(gh);
        assert_eq!(full.number, 7);
        assert_eq!(full.title, "Fix");
        assert_eq!(full.body, "body");
        assert_eq!(full.state, "open");
        assert_eq!(full.state_reason.as_deref(), Some("reopened"));
        assert_eq!(full.labels, vec!["bug".to_owned()]);
        assert_eq!(full.assignee.as_deref(), Some("alice"));
        assert_eq!(full.milestone.as_deref(), Some("v1"));
        assert_eq!(full.updated_at, "2026-09-12T00:00:00Z");
        assert_eq!(full.url, "https://github.com/o/r/issues/7");
    }

    #[test]
    fn null_body_and_empty_labels_tolerated() {
        let gh = GhIssue {
            number: 8,
            title: "No body".to_owned(),
            body: None,
            state: "closed".to_owned(),
            state_reason: None,
            labels: vec![],
            assignee: None,
            milestone: None,
            updated_at: "2026-09-12T00:00:00Z".to_owned(),
            html_url: "https://github.com/o/r/issues/8".to_owned(),
        };

        let full = IssueFull::from(gh);
        assert_eq!(full.body, "");
        assert!(full.labels.is_empty());
        assert!(full.assignee.is_none());
        assert!(full.milestone.is_none());
        assert!(full.state_reason.is_none());
    }
}
