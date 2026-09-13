//! Integration: keybindings + action dispatch against fake data-layer deps.

use std::cell::{Cell, RefCell};

use crossterm::event::{KeyCode, KeyModifiers};

use herdr_board::{
    create_draft, dispatch, sync, Action, App, Candidate, CanonicalRequest, Conflict, CreateIntent,
    CreateResult, Deps, ExternalResponse, FactoryKind, GitHubClient, HandoffResult,
    HandoffTransport, Identity, Issue, IssueFull, IssuePatch, KeyMap, PullClient, RepoIdentity,
    Store, UpdateResult,
};

struct FakeClient {
    issues: RefCell<Vec<IssueFull>>,
    list_calls: Cell<usize>,
    fetch_calls: Cell<usize>,
    create_calls: Cell<usize>,
    update_calls: Cell<usize>,
    next_number: Cell<u64>,
}

impl FakeClient {
    fn new(issues: Vec<IssueFull>) -> Self {
        let next = issues.iter().map(|i| i.number).max().unwrap_or(0) + 1;
        FakeClient {
            issues: RefCell::new(issues),
            list_calls: Cell::new(0),
            fetch_calls: Cell::new(0),
            create_calls: Cell::new(0),
            update_calls: Cell::new(0),
            next_number: Cell::new(next),
        }
    }
}

impl PullClient for FakeClient {
    fn list_issues(&self, _repo: &RepoIdentity) -> Vec<IssueFull> {
        self.list_calls.set(self.list_calls.get() + 1);
        self.issues.borrow().clone()
    }
    fn fetch_issue(&self, _repo: &RepoIdentity, number: u64) -> Option<IssueFull> {
        self.fetch_calls.set(self.fetch_calls.get() + 1);
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
        self.create_calls.set(self.create_calls.get() + 1);
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
        self.update_calls.set(self.update_calls.get() + 1);
        UpdateResult::Updated {
            updated_at: "r2".to_owned(),
        }
    }
}

struct FakeTransport {
    calls: Cell<usize>,
}

impl HandoffTransport for FakeTransport {
    fn handoff(&self, _request: &CanonicalRequest) -> HandoffResult {
        self.calls.set(self.calls.get() + 1);
        HandoffResult::Accepted(ExternalResponse::new("ok"))
    }
}

fn repo() -> RepoIdentity {
    RepoIdentity::new("ThalixInc", "herdr-board")
}

fn id(number: u64) -> Identity {
    Identity::new("thalixinc", "herdr-board", number)
}

fn issue(number: u64, title: &str, updated_at: &str) -> IssueFull {
    IssueFull {
        number,
        title: title.to_owned(),
        body: "body".to_owned(),
        state: "open".to_owned(),
        state_reason: None,
        labels: vec![],
        assignee: None,
        milestone: None,
        updated_at: updated_at.to_owned(),
        url: format!("https://github.com/thalixinc/herdr-board/issues/{number}"),
    }
}

#[test]
fn keymap_resolves_bindings() {
    let map = KeyMap;
    assert_eq!(
        map.resolve(KeyCode::Char('s'), KeyModifiers::NONE),
        Some(Action::Sync)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('a'), KeyModifiers::NONE),
        Some(Action::ApplyPull)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('d'), KeyModifiers::NONE),
        Some(Action::DeferPull)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('p'), KeyModifiers::NONE),
        Some(Action::Publish)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('A'), KeyModifiers::SHIFT),
        Some(Action::ApplyPush)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('D'), KeyModifiers::SHIFT),
        Some(Action::DiscardPush)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('f'), KeyModifiers::NONE),
        Some(Action::ProcessFactory)
    );
    assert_eq!(
        map.resolve(KeyCode::Left, KeyModifiers::NONE),
        Some(Action::MoveLeft)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('h'), KeyModifiers::NONE),
        Some(Action::MoveLeft)
    );
    assert_eq!(
        map.resolve(KeyCode::Right, KeyModifiers::NONE),
        Some(Action::MoveRight)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('l'), KeyModifiers::NONE),
        Some(Action::MoveRight)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('q'), KeyModifiers::NONE),
        Some(Action::Quit)
    );
    assert_eq!(
        map.resolve(KeyCode::Esc, KeyModifiers::NONE),
        Some(Action::Quit)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('c'), KeyModifiers::CONTROL),
        Some(Action::Quit)
    );
    assert_eq!(map.resolve(KeyCode::Char('x'), KeyModifiers::NONE), None);
}

#[test]
fn sync_action_calls_sync_and_reloads() {
    let client = FakeClient::new(vec![issue(1, "Fix sync", "r1")]);
    let transport = FakeTransport {
        calls: Cell::new(0),
    };
    let repo = repo();
    let store = Store::open_in_memory().unwrap();
    let deps = Deps {
        pull: &client,
        github: &client,
        transport: &transport,
        repo: &repo,
    };
    let mut app = App::new(store, repo.clone());

    dispatch(&mut app, Action::Sync, &deps).unwrap();

    assert_eq!(client.list_calls.get(), 1);
    assert_eq!(app.model.len(), 1, "model reloaded after sync");
}

#[test]
fn apply_pull_clears_conflict() {
    let client = FakeClient::new(vec![issue(1, "Fix sync", "r1")]);
    let transport = FakeTransport {
        calls: Cell::new(0),
    };
    let repo = repo();
    let store = Store::open_in_memory().unwrap();
    sync(&store, &client, &repo).unwrap();
    store.record_conflict(&id(1), &[], 0).unwrap();

    let deps = Deps {
        pull: &client,
        github: &client,
        transport: &transport,
        repo: &repo,
    };
    let mut app = App::new(store, repo.clone());

    dispatch(&mut app, Action::ApplyPull, &deps).unwrap();
    assert_eq!(
        app.store.get_card(&id(1)).unwrap().unwrap().conflict,
        Conflict::None
    );
}

#[test]
fn defer_pull_keeps_conflict() {
    let client = FakeClient::new(vec![issue(1, "Fix sync", "r1")]);
    let transport = FakeTransport {
        calls: Cell::new(0),
    };
    let repo = repo();
    let store = Store::open_in_memory().unwrap();
    sync(&store, &client, &repo).unwrap();
    store.record_conflict(&id(1), &[], 0).unwrap();

    let deps = Deps {
        pull: &client,
        github: &client,
        transport: &transport,
        repo: &repo,
    };
    let mut app = App::new(store, repo.clone());

    dispatch(&mut app, Action::DeferPull, &deps).unwrap();
    assert_eq!(
        app.store.get_card(&id(1)).unwrap().unwrap().conflict,
        Conflict::ApplyPending
    );
}

#[test]
fn publish_action_creates_issue() {
    let client = FakeClient::new(vec![]);
    let transport = FakeTransport {
        calls: Cell::new(0),
    };
    let repo = repo();
    let store = Store::open_in_memory().unwrap();
    create_draft(&store, FactoryKind::Ordinary, "Draft", "body").unwrap();

    let deps = Deps {
        pull: &client,
        github: &client,
        transport: &transport,
        repo: &repo,
    };
    let mut app = App::new(store, repo.clone());

    dispatch(&mut app, Action::Publish, &deps).unwrap();
    assert_eq!(client.create_calls.get(), 1);
}

#[test]
fn process_action_hands_off() {
    let client = FakeClient::new(vec![issue(1, "Fix sync", "r1")]);
    let transport = FakeTransport {
        calls: Cell::new(0),
    };
    let repo = repo();
    let store = Store::open_in_memory().unwrap();
    create_draft(&store, FactoryKind::Ordinary, "Draft", "body").unwrap();
    sync(&store, &client, &repo).unwrap();

    let deps = Deps {
        pull: &client,
        github: &client,
        transport: &transport,
        repo: &repo,
    };
    let mut app = App::new(store, repo.clone());

    dispatch(&mut app, Action::ProcessFactory, &deps).unwrap();
    assert_eq!(transport.calls.get(), 1);
}

#[test]
fn move_action_sets_column_without_github_write() {
    let client = FakeClient::new(vec![issue(1, "Fix sync", "r1")]);
    let transport = FakeTransport {
        calls: Cell::new(0),
    };
    let repo = repo();
    let store = Store::open_in_memory().unwrap();
    sync(&store, &client, &repo).unwrap();

    let deps = Deps {
        pull: &client,
        github: &client,
        transport: &transport,
        repo: &repo,
    };
    let mut app = App::new(store, repo.clone());

    dispatch(&mut app, Action::MoveRight, &deps).unwrap();
    assert_eq!(
        app.store.get_card(&id(1)).unwrap().unwrap().column,
        "in-progress"
    );
    assert_eq!(client.create_calls.get(), 0, "move must not write GitHub");
    assert_eq!(client.update_calls.get(), 0, "move must not write GitHub");
}
