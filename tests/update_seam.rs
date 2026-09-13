//! The publish/update write seam: `IssuePatch` tri-state semantics and the
//! three-way `UpdateResult`, exercised against a fake `GitHubClient`.

use std::cell::RefCell;

use herdr_board::{
    Candidate, CreateIntent, CreateResult, GitHubClient, Issue, IssuePatch, RepoIdentity,
    UpdateResult,
};

/// The board-visible fields a fake issue holds, so a test can observe how a
/// patch was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IssueFields {
    title: String,
    body: String,
    labels: Vec<String>,
    assignee: Option<String>,
    milestone: Option<String>,
}

fn fields() -> IssueFields {
    IssueFields {
        title: "t".to_string(),
        body: "b".to_string(),
        labels: vec!["bug".to_string()],
        assignee: Some("alice".to_string()),
        milestone: Some("v1".to_string()),
    }
}

/// A fake client that applies each patch to an in-memory issue and returns a
/// fixed `UpdateResult`.
struct FakeClient {
    current: RefCell<IssueFields>,
    update: UpdateResult,
}

impl FakeClient {
    fn new(current: IssueFields, update: UpdateResult) -> Self {
        FakeClient {
            current: RefCell::new(current),
            update,
        }
    }
}

impl GitHubClient for FakeClient {
    fn create_issue(&self, _repo: &RepoIdentity, _i: &CreateIntent, _m: &str) -> CreateResult {
        CreateResult::Created(1)
    }

    fn search_issues(&self, _repo: &RepoIdentity, _query: &str) -> Vec<Candidate> {
        Vec::new()
    }

    fn get_issue(&self, _repo: &RepoIdentity, _number: u64) -> Option<Issue> {
        None
    }

    fn update_issue(&self, _repo: &RepoIdentity, _number: u64, patch: &IssuePatch) -> UpdateResult {
        let mut cur = self.current.borrow_mut();
        // Single-`Option` (title/body/labels): replace-on-set, `None` leaves.
        if let Some(title) = &patch.title {
            cur.title = title.clone();
        }
        if let Some(body) = &patch.body {
            cur.body = body.clone();
        }
        if let Some(labels) = &patch.labels {
            cur.labels = labels.clone(); // labels replace the whole set
        }
        // Double-`Option` (assignee/milestone): `Some(None)` clears, `Some(Some)` sets.
        if let Some(assignee) = &patch.assignee {
            cur.assignee = assignee.clone();
        }
        if let Some(milestone) = &patch.milestone {
            cur.milestone = milestone.clone();
        }
        self.update.clone()
    }
}

#[test]
fn assignee_tristate() {
    let repo = RepoIdentity::new("o", "r");
    let updated = UpdateResult::Updated {
        updated_at: "r1".to_string(),
    };
    let client = FakeClient::new(fields(), updated);

    // None leaves unchanged.
    client.update_issue(&repo, 1, &IssuePatch::default());
    assert_eq!(client.current.borrow().assignee, Some("alice".to_string()));

    // Some(None) clears.
    client.update_issue(
        &repo,
        1,
        &IssuePatch {
            assignee: Some(None),
            ..IssuePatch::default()
        },
    );
    assert_eq!(client.current.borrow().assignee, None);

    // Some(Some(v)) sets.
    client.update_issue(
        &repo,
        1,
        &IssuePatch {
            assignee: Some(Some("bob".to_string())),
            ..IssuePatch::default()
        },
    );
    assert_eq!(client.current.borrow().assignee, Some("bob".to_string()));
}

#[test]
fn labels_replace_set_and_leave() {
    let repo = RepoIdentity::new("o", "r");
    let updated = UpdateResult::Updated {
        updated_at: "r1".to_string(),
    };
    let client = FakeClient::new(fields(), updated);

    // labels replace the whole set.
    client.update_issue(
        &repo,
        1,
        &IssuePatch {
            labels: Some(vec!["x".to_string(), "y".to_string()]),
            ..IssuePatch::default()
        },
    );
    assert_eq!(
        client.current.borrow().labels,
        vec!["x".to_string(), "y".to_string()]
    );

    // None leaves the set unchanged.
    client.update_issue(&repo, 1, &IssuePatch::default());
    assert_eq!(
        client.current.borrow().labels,
        vec!["x".to_string(), "y".to_string()]
    );
}

#[test]
fn title_body_replace_or_leave() {
    let repo = RepoIdentity::new("o", "r");
    let updated = UpdateResult::Updated {
        updated_at: "r1".to_string(),
    };
    let client = FakeClient::new(fields(), updated);

    client.update_issue(
        &repo,
        1,
        &IssuePatch {
            title: Some("new title".to_string()),
            body: Some("new body".to_string()),
            ..IssuePatch::default()
        },
    );
    assert_eq!(client.current.borrow().title, "new title");
    assert_eq!(client.current.borrow().body, "new body");

    client.update_issue(&repo, 1, &IssuePatch::default());
    assert_eq!(client.current.borrow().title, "new title");
    assert_eq!(client.current.borrow().body, "new body");
}

#[test]
fn update_result_three_way() {
    let repo = RepoIdentity::new("o", "r");
    let patch = IssuePatch::default();

    let updated = FakeClient::new(
        fields(),
        UpdateResult::Updated {
            updated_at: "r2".to_string(),
        },
    );
    match updated.update_issue(&repo, 1, &patch) {
        UpdateResult::Updated { updated_at } => assert_eq!(updated_at, "r2"),
        other => panic!("expected Updated, got {other:?}"),
    }

    let failed = FakeClient::new(fields(), UpdateResult::Failed("no permission".to_string()));
    match failed.update_issue(&repo, 1, &patch) {
        UpdateResult::Failed(msg) => assert_eq!(msg, "no permission"),
        other => panic!("expected Failed, got {other:?}"),
    }

    let uncertain = FakeClient::new(fields(), UpdateResult::Uncertain("timeout".to_string()));
    match uncertain.update_issue(&repo, 1, &patch) {
        UpdateResult::Uncertain(msg) => assert_eq!(msg, "timeout"),
        other => panic!("expected Uncertain, got {other:?}"),
    }
}
