//! Create orchestration lifecycle against a fake GitHub client: clean create,
//! lost response, block-auto-replay, manual link, cancel, and candidate search.

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use herdr_board::{
    cancel, candidates, issue, link, marker_comment, Candidate, CreateError, CreateIntent,
    CreateOutcome, CreateResult, GitHubClient, Issue, Marker, RepoIdentity, Store,
};

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64
}

fn draft() -> CreateIntent {
    CreateIntent::new(
        Marker::new_v4(),
        "thalixinc/herdr-board".to_string(),
        "Fix the board sync".to_string(),
        "The board should sync issues".to_string(),
        "[\"bug\"]".to_string(),
    )
}

/// A fake GitHub client: returns a fixed [`CreateResult`], tracks created
/// issues for `get_issue`/`search_issues`, and counts create calls.
struct FakeGitHubClient {
    create: CreateResult,
    issues: RefCell<Vec<IssueRecord>>,
    create_calls: AtomicUsize,
}

struct IssueRecord {
    number: u64,
    title: String,
    body: String,
    created_at: i64,
}

impl FakeGitHubClient {
    fn new(create: CreateResult) -> Self {
        FakeGitHubClient {
            create,
            issues: RefCell::new(Vec::new()),
            create_calls: AtomicUsize::new(0),
        }
    }

    fn seed(&self, number: u64, title: &str, body: &str) {
        self.issues.borrow_mut().push(IssueRecord {
            number,
            title: title.to_string(),
            body: body.to_string(),
            created_at: now_unix(),
        });
    }

    fn create_calls(&self) -> usize {
        self.create_calls.load(Ordering::SeqCst)
    }
}

impl GitHubClient for FakeGitHubClient {
    fn create_issue(
        &self,
        _repo: &RepoIdentity,
        intent: &CreateIntent,
        marker_comment: &str,
    ) -> CreateResult {
        self.create_calls.fetch_add(1, Ordering::SeqCst);
        match &self.create {
            CreateResult::Created(n) => {
                let body = format!("{}{}", intent.body, marker_comment);
                self.issues.borrow_mut().push(IssueRecord {
                    number: *n,
                    title: intent.title.clone(),
                    body,
                    created_at: now_unix(),
                });
                CreateResult::Created(*n)
            }
            CreateResult::Failed(e) => CreateResult::Failed(e.clone()),
            CreateResult::Uncertain(e) => CreateResult::Uncertain(e.clone()),
        }
    }

    fn search_issues(&self, _repo: &RepoIdentity, query: &str) -> Vec<Candidate> {
        self.issues
            .borrow()
            .iter()
            .filter(|i| {
                i.body.contains(query)
                    || i.title
                        .to_ascii_lowercase()
                        .contains(&query.to_ascii_lowercase())
            })
            .map(|i| Candidate {
                number: i.number,
                title: i.title.clone(),
                body_snippet: i.body.chars().take(80).collect(),
                created_at: Some(i.created_at),
            })
            .collect()
    }

    fn get_issue(&self, _repo: &RepoIdentity, number: u64) -> Option<Issue> {
        self.issues
            .borrow()
            .iter()
            .find(|i| i.number == number)
            .map(|i| Issue {
                number: i.number,
                title: i.title.clone(),
            })
    }
}

#[test]
fn clean_create_sets_issue_number() {
    let store = Store::open_in_memory().expect("open store");
    let client = FakeGitHubClient::new(CreateResult::Created(4242));
    let intent = draft();

    let outcome = issue(&store, &client, &intent).expect("issue ok");
    assert_eq!(outcome, CreateOutcome::Created);

    let stored = store
        .get_by_marker(&intent.marker)
        .expect("get by marker")
        .expect("intent stored");
    assert_eq!(stored.outcome, CreateOutcome::Created);
    assert_eq!(stored.issue_number, Some(4242));
    assert!(stored.finalized_at.is_some());
}

#[test]
fn lost_response_stays_issued() {
    let store = Store::open_in_memory().expect("open store");
    let client = FakeGitHubClient::new(CreateResult::Uncertain("timeout".to_string()));
    let intent = draft();

    let result = issue(&store, &client, &intent);
    assert!(matches!(result, Err(CreateError::Uncertain(_))));

    let stored = store
        .get_by_marker(&intent.marker)
        .expect("get by marker")
        .expect("intent stored");
    assert_eq!(stored.outcome, CreateOutcome::Issued);
    assert!(stored.issue_number.is_none());
    assert!(stored.finalized_at.is_none());

    // It surfaces in the startup sweep as "awaiting link".
    assert_eq!(store.list_issued().expect("list issued").len(), 1);
}

#[test]
fn block_auto_replay_sends_exactly_one_create() {
    let store = Store::open_in_memory().expect("open store");
    let client = FakeGitHubClient::new(CreateResult::Uncertain("timeout".to_string()));
    let intent = draft();

    let first = issue(&store, &client, &intent);
    assert!(matches!(first, Err(CreateError::Uncertain(_))));

    // Re-issuing the same intent is refused — no second create call.
    let second = issue(&store, &client, &intent);
    assert!(second.is_err());
    assert_eq!(client.create_calls(), 1);
}

#[test]
fn link_resolves_issued_to_created() {
    let store = Store::open_in_memory().expect("open store");
    let client = FakeGitHubClient::new(CreateResult::Uncertain("timeout".to_string()));
    client.seed(999, "Fix the board sync", "no marker");

    let intent = draft();
    assert!(issue(&store, &client, &intent).is_err());

    let outcome = link(&store, &client, &intent.intent_id, 999).expect("link ok");
    assert_eq!(outcome, CreateOutcome::Created);

    let stored = store
        .get_by_marker(&intent.marker)
        .expect("get by marker")
        .expect("intent stored");
    assert_eq!(stored.outcome, CreateOutcome::Created);
    assert_eq!(stored.issue_number, Some(999));
}

#[test]
fn link_refuses_nonexistent_issue() {
    let store = Store::open_in_memory().expect("open store");
    let client = FakeGitHubClient::new(CreateResult::Uncertain("timeout".to_string()));
    let intent = draft();
    assert!(issue(&store, &client, &intent).is_err());

    // Issue 12345 does not exist on the client → link refuses.
    assert!(matches!(
        link(&store, &client, &intent.intent_id, 12345),
        Err(CreateError::NotFound)
    ));
}

#[test]
fn cancel_stops_tracking() {
    let store = Store::open_in_memory().expect("open store");
    let client = FakeGitHubClient::new(CreateResult::Uncertain("timeout".to_string()));
    let intent = draft();
    assert!(issue(&store, &client, &intent).is_err());

    let outcome = cancel(&store, &intent.intent_id).expect("cancel ok");
    assert_eq!(outcome, CreateOutcome::Cancelled);

    let stored = store
        .get_by_marker(&intent.marker)
        .expect("get by marker")
        .expect("intent stored");
    assert_eq!(stored.outcome, CreateOutcome::Cancelled);
}

#[test]
fn candidates_prefer_marker_match() {
    let store = Store::open_in_memory().expect("open store");
    let client = FakeGitHubClient::new(CreateResult::Uncertain("timeout".to_string()));

    let intent = draft();
    store.insert_intent(&intent).expect("insert intent");
    store.mark_issued(&intent.intent_id).expect("mark issued");

    // The real issue carries the marker comment in its body.
    let comment = marker_comment(&intent.marker);
    let body = format!("sync body {comment}");
    client.seed(777, "Fix the board sync", &body);

    let found = candidates(&store, &client, &intent.intent_id).expect("candidates");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].number, 777);
}

#[test]
fn candidates_fall_back_to_normalized_title() {
    let store = Store::open_in_memory().expect("open store");
    let client = FakeGitHubClient::new(CreateResult::Uncertain("timeout".to_string()));

    let mut intent = draft();
    intent.title = "  Fix   THE Board\nSync  ".to_string(); // weird case/whitespace
    store.insert_intent(&intent).expect("insert intent");
    store.mark_issued(&intent.intent_id).expect("mark issued");

    // No marker in this issue's body; matches by normalized title.
    client.seed(888, "Fix the Board Sync", "no marker here");

    let found = candidates(&store, &client, &intent.intent_id).expect("candidates");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].number, 888);
}
