//! Runnable smoke: draft → publish → linked card, edit → pending write,
//! drifted push → conflict diff, apply → written.

use std::cell::{Cell, RefCell, RefMut};

use herdr_board::{
    apply_push, create_draft, publish_draft, push_changes, Candidate, CreateIntent, CreateResult,
    FactoryKind, GitHubClient, Issue, IssueFull, IssuePatch, PendingWrite, PullClient, PushError,
    RepoIdentity, Store, UpdateResult, WriteOutcome,
};

struct FakeClient {
    issues: RefCell<Vec<IssueFull>>,
    next_number: Cell<u64>,
    update_result: RefCell<UpdateResult>,
}

impl FakeClient {
    fn issue_mut(&self, number: u64) -> RefMut<'_, IssueFull> {
        RefMut::map(self.issues.borrow_mut(), |v| {
            v.iter_mut().find(|i| i.number == number).unwrap()
        })
    }
}

impl PullClient for FakeClient {
    fn list_issues(&self, _repo: &RepoIdentity) -> Vec<IssueFull> {
        self.issues.borrow().clone()
    }
    fn fetch_issue(&self, _repo: &RepoIdentity, number: u64) -> Option<IssueFull> {
        self.issues
            .borrow()
            .iter()
            .find(|i| i.number == number)
            .cloned()
    }
}

impl GitHubClient for FakeClient {
    fn create_issue(
        &self,
        _repo: &RepoIdentity,
        intent: &CreateIntent,
        _marker: &str,
    ) -> CreateResult {
        let number = self.next_number.get();
        self.next_number.set(number + 1);
        let labels = serde_json::from_str::<Vec<String>>(&intent.labels).unwrap_or_default();
        self.issues.borrow_mut().push(IssueFull {
            number,
            title: intent.title.clone(),
            body: intent.body.clone(),
            state: "open".to_owned(),
            state_reason: None,
            labels,
            assignee: intent.assignee.clone(),
            milestone: None,
            updated_at: "r1".to_owned(),
            url: format!("https://github.com/o/r/issues/{number}"),
        });
        CreateResult::Created(number)
    }
    fn search_issues(&self, _repo: &RepoIdentity, _query: &str) -> Vec<Candidate> {
        Vec::new()
    }
    fn get_issue(&self, _repo: &RepoIdentity, _number: u64) -> Option<Issue> {
        None
    }
    fn update_issue(
        &self,
        _repo: &RepoIdentity,
        _number: u64,
        _patch: &IssuePatch,
    ) -> UpdateResult {
        self.update_result.borrow().clone()
    }
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "herdr-board-push-demo-{}-{}",
        std::process::id(),
        herdr_board::HandoffId::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn main() {
    let dir = temp_dir();
    let store = Store::open(dir.join("herdr-board.sqlite3")).expect("open store");
    let repo = RepoIdentity::new("ThalixInc", "herdr-board");
    let client = FakeClient {
        issues: RefCell::new(Vec::new()),
        next_number: Cell::new(1),
        update_result: RefCell::new(UpdateResult::Updated {
            updated_at: "r2".to_owned(),
        }),
    };

    // 1. Draft → publish → linked card.
    let draft = create_draft(&store, FactoryKind::Ordinary, "Fix sync", "pull issues").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo, &[], None).unwrap();
    println!(
        "published: {} revision={}",
        card.identity.canonical(),
        card.revision
    );

    // 2. Edit → pending write.
    let identity = card.identity.clone();
    store
        .record_write(&PendingWrite {
            identity: identity.clone(),
            base_revision: card.revision.clone(),
            title: Some("Fix sync (edited)".to_owned()),
            body: None,
            labels: None,
            assignee: None,
            milestone: None,
            outcome: WriteOutcome::Pending,
        })
        .unwrap();
    println!("edit: pending write recorded");

    // 3. GitHub drifts; push surfaces a conflict.
    client.issue_mut(1).updated_at = "r3".to_owned();
    client.issue_mut(1).title = "Fix sync (GitHub changed)".to_owned();
    match push_changes(&store, &client, &identity) {
        Err(PushError::Conflict { diffs }) => {
            println!("push: CONFLICT");
            for diff in &diffs {
                println!(
                    "  {}: {:?} -> {:?}",
                    diff.field.as_str(),
                    diff.old,
                    diff.new
                );
            }
        }
        other => panic!("expected conflict, got {other:?}"),
    }

    // 4. Human applies (board wins) → written.
    let card = apply_push(&store, &client, &identity).unwrap();
    println!(
        "applied: conflict={} title={:?} revision={}",
        card.conflict.as_str(),
        card.fields.title,
        card.revision
    );

    let _ = std::fs::remove_dir_all(&dir);
}
