//! The read-only GitHub pull client: the injectable seam for fetching issues,
//! plus the real `ureq` implementation.

use std::path::PathBuf;

use serde::Deserialize;

use crate::create::{
    Candidate, CreateResult, GitHubClient, Issue, IssuePatch, RepoIdentity, UpdateResult,
};
use crate::outbox::CreateIntent;

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

    fn with_headers(&self, req: ureq::Request) -> ureq::Request {
        let mut req = req
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", "herdr-board")
            .set("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
        req
    }

    /// Send a JSON write and classify the outcome: a 4xx/5xx is a definite
    /// failure; a transport error (no response) is uncertain.
    fn send_write(
        &self,
        req: ureq::Request,
        payload: serde_json::Value,
    ) -> Result<String, WriteFailure> {
        let body = serde_json::to_string(&payload).expect("json value always serializes");
        let req = self
            .with_headers(req)
            .set("Content-Type", "application/json");
        match req.send_string(&body) {
            Ok(response) => Ok(response.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(code, _)) => {
                Err(WriteFailure::Definite(format!("HTTP {code}")))
            }
            Err(ureq::Error::Transport(t)) => Err(WriteFailure::Uncertain(t.to_string())),
        }
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

impl GitHubClient for RealGitHubClient {
    fn create_issue(
        &self,
        repo: &RepoIdentity,
        intent: &CreateIntent,
        marker_comment: &str,
    ) -> CreateResult {
        let url = format!(
            "{}/repos/{}/{}/issues",
            self.api_base, repo.owner, repo.repo
        );
        let labels: serde_json::Value = serde_json::from_str(&intent.labels)
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
        let payload = serde_json::json!({
            "title": intent.title.as_str(),
            "body": format!("{}{}", intent.body, marker_comment),
            "labels": labels,
        });
        match self.send_write(ureq::post(&url), payload) {
            Ok(text) => match serde_json::from_str::<GhIssue>(&text) {
                Ok(issue) => CreateResult::Created(issue.number),
                Err(e) => CreateResult::Failed(format!("bad create response: {e}")),
            },
            Err(WriteFailure::Definite(msg)) => CreateResult::Failed(msg),
            Err(WriteFailure::Uncertain(msg)) => CreateResult::Uncertain(msg),
        }
    }

    fn search_issues(&self, repo: &RepoIdentity, query: &str) -> Vec<Candidate> {
        let q = format!("repo:{}/{} {}", repo.owner, repo.repo, query);
        let path = format!("/search/issues?q={}", urlencode(&q));
        let text = match self.get_text(&path) {
            Some(text) => text,
            None => return Vec::new(),
        };
        let response: GhSearchResponse = match serde_json::from_str(&text) {
            Ok(response) => response,
            Err(_) => return Vec::new(),
        };
        response
            .items
            .into_iter()
            .map(|issue| Candidate {
                number: issue.number,
                title: issue.title,
                body_snippet: issue.body.unwrap_or_default().chars().take(80).collect(),
                created_at: None,
            })
            .collect()
    }

    fn get_issue(&self, repo: &RepoIdentity, number: u64) -> Option<Issue> {
        let path = format!("/repos/{}/{}/issues/{number}", repo.owner, repo.repo);
        let text = self.get_text(&path)?;
        let issue: GhIssue = serde_json::from_str(&text).ok()?;
        Some(Issue {
            number: issue.number,
            title: issue.title,
        })
    }

    fn update_issue(&self, repo: &RepoIdentity, number: u64, patch: &IssuePatch) -> UpdateResult {
        let url = format!(
            "{}/repos/{}/{}/issues/{number}",
            self.api_base, repo.owner, repo.repo
        );
        let payload = patch_to_json(patch);
        match self.send_write(ureq::patch(&url), payload) {
            Ok(text) => match serde_json::from_str::<GhIssue>(&text) {
                Ok(issue) => UpdateResult::Updated {
                    updated_at: issue.updated_at,
                },
                Err(e) => UpdateResult::Failed(format!("bad update response: {e}")),
            },
            Err(WriteFailure::Definite(msg)) => UpdateResult::Failed(msg),
            Err(WriteFailure::Uncertain(msg)) => UpdateResult::Uncertain(msg),
        }
    }
}

/// A write failed with a definite rejection, or was lost (uncertain).
enum WriteFailure {
    Definite(String),
    Uncertain(String),
}

/// Minimal RFC-3986 percent-encoding for the search query (unreserved chars
/// pass through; everything else is escaped).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the PATCH body from an [`IssuePatch`]: only present fields are sent;
/// assignee/milestone map `Some(None)` → JSON `null` (clear).
fn patch_to_json(patch: &IssuePatch) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if let Some(title) = &patch.title {
        map.insert("title".to_owned(), serde_json::Value::String(title.clone()));
    }
    if let Some(body) = &patch.body {
        map.insert("body".to_owned(), serde_json::Value::String(body.clone()));
    }
    if let Some(labels) = &patch.labels {
        map.insert(
            "labels".to_owned(),
            serde_json::Value::Array(
                labels
                    .iter()
                    .map(|l| serde_json::Value::String(l.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(assignee) = &patch.assignee {
        map.insert(
            "assignee".to_owned(),
            match assignee {
                Some(name) => serde_json::Value::String(name.clone()),
                None => serde_json::Value::Null,
            },
        );
    }
    if let Some(milestone) = &patch.milestone {
        map.insert(
            "milestone".to_owned(),
            match milestone {
                Some(title) => serde_json::Value::String(title.clone()),
                None => serde_json::Value::Null,
            },
        );
    }
    serde_json::Value::Object(map)
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

/// The wire shape of a GitHub search response (the `items` slice).
#[derive(Debug, Deserialize)]
struct GhSearchResponse {
    #[serde(default)]
    items: Vec<GhIssue>,
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

    #[test]
    fn patch_to_json_tristate() {
        // Empty patch → no keys.
        assert_eq!(patch_to_json(&IssuePatch::default()), serde_json::json!({}));

        // Set values (single-Option present).
        let set = patch_to_json(&IssuePatch {
            title: Some("t".to_owned()),
            labels: Some(vec!["a".to_owned()]),
            assignee: Some(Some("bob".to_owned())),
            milestone: Some(Some("v1".to_owned())),
            ..IssuePatch::default()
        });
        assert_eq!(
            set,
            serde_json::json!({
                "title": "t",
                "labels": ["a"],
                "assignee": "bob",
                "milestone": "v1",
            })
        );

        // Clear values (double-Option Some(None) → JSON null).
        let clear = patch_to_json(&IssuePatch {
            assignee: Some(None),
            milestone: Some(None),
            ..IssuePatch::default()
        });
        assert_eq!(
            clear,
            serde_json::json!({
                "assignee": null,
                "milestone": null,
            })
        );
    }
}
