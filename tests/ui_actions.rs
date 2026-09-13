//! Integration: keybindings + action dispatch against fake data-layer deps.

use std::cell::{Cell, RefCell};

use crossterm::event::{KeyCode, KeyModifiers};

use herdr_board::{
    create_draft, dispatch, sync, Action, App, Candidate, CanonicalRequest, Conflict, CreateIntent,
    CreateResult, Deps, ExternalResponse, FactoryKind, FilterDimension, FilterInput, GitHubClient,
    HandoffResult, HandoffTransport, Identity, Issue, IssueFull, IssuePatch, KeyMap, PullClient,
    RepoIdentity, Store, UpdateResult,
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

fn issue_with_assignee(number: u64, title: &str, assignee: &str) -> IssueFull {
    let mut issue = issue(number, title, "r1");
    issue.assignee = Some(assignee.to_owned());
    issue
}

#[test]
fn keymap_resolves_bindings() {
    let map = KeyMap;
    // Bar closed: normal board bindings.
    assert_eq!(
        map.resolve(KeyCode::Char('s'), KeyModifiers::NONE, false),
        Some(Action::Sync)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('a'), KeyModifiers::NONE, false),
        Some(Action::ApplyPull)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('d'), KeyModifiers::NONE, false),
        Some(Action::DeferPull)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('p'), KeyModifiers::NONE, false),
        Some(Action::Publish)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('A'), KeyModifiers::SHIFT, false),
        Some(Action::ApplyPush)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('D'), KeyModifiers::SHIFT, false),
        Some(Action::DiscardPush)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('f'), KeyModifiers::NONE, false),
        Some(Action::ProcessFactory)
    );
    assert_eq!(
        map.resolve(KeyCode::Left, KeyModifiers::NONE, false),
        Some(Action::MoveLeft)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('h'), KeyModifiers::NONE, false),
        Some(Action::MoveLeft)
    );
    assert_eq!(
        map.resolve(KeyCode::Right, KeyModifiers::NONE, false),
        Some(Action::MoveRight)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('l'), KeyModifiers::NONE, false),
        Some(Action::MoveRight)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('q'), KeyModifiers::NONE, false),
        Some(Action::Quit)
    );
    assert_eq!(
        map.resolve(KeyCode::Esc, KeyModifiers::NONE, false),
        Some(Action::Quit)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('c'), KeyModifiers::CONTROL, false),
        Some(Action::Quit)
    );
    assert_eq!(
        map.resolve(KeyCode::Char('x'), KeyModifiers::NONE, false),
        None
    );
    // `/` opens the bar; it never clears filters.
    assert_eq!(
        map.resolve(KeyCode::Char('/'), KeyModifiers::NONE, false),
        Some(Action::FilterOpen)
    );

    // Bar open: typing/navigation bindings take over.
    assert_eq!(
        map.resolve(KeyCode::Char('s'), KeyModifiers::NONE, true),
        Some(Action::FilterChar('s')),
        "characters type while the bar is open"
    );
    assert_eq!(
        map.resolve(KeyCode::Char('x'), KeyModifiers::NONE, true),
        Some(Action::ClearFilters),
        "x clears all filters while the bar is open"
    );
    assert_eq!(
        map.resolve(KeyCode::Enter, KeyModifiers::NONE, true),
        Some(Action::FilterApply)
    );
    assert_eq!(
        map.resolve(KeyCode::Backspace, KeyModifiers::NONE, true),
        Some(Action::FilterBackspace)
    );
    assert_eq!(
        map.resolve(KeyCode::Delete, KeyModifiers::NONE, true),
        Some(Action::FilterClearDimension)
    );
    assert_eq!(
        map.resolve(KeyCode::Tab, KeyModifiers::NONE, true),
        Some(Action::FilterNext)
    );
    assert_eq!(
        map.resolve(KeyCode::Left, KeyModifiers::NONE, true),
        Some(Action::FilterNext),
        "left cycles dimension while the bar is open"
    );
    assert_eq!(
        map.resolve(KeyCode::Right, KeyModifiers::NONE, true),
        Some(Action::FilterNext)
    );
    assert_eq!(
        map.resolve(KeyCode::Esc, KeyModifiers::NONE, true),
        Some(Action::FilterClose),
        "esc closes the bar without applying"
    );
    assert_eq!(
        map.resolve(KeyCode::Char('c'), KeyModifiers::CONTROL, true),
        Some(Action::Quit),
        "ctrl+c still quits while the bar is open"
    );
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

#[test]
fn filter_input_apply_clear_and_cycle() {
    let client = FakeClient::new(vec![
        issue_with_assignee(1, "Founder's task", "founder"),
        issue_with_assignee(2, "Other's task", "other"),
    ]);
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
    assert_eq!(app.model.len(), 2);

    // `/` opens the bar on the assignee dimension.
    dispatch(&mut app, Action::FilterOpen, &deps).unwrap();
    assert!(app.filter_input.is_active());
    assert!(matches!(
        &app.filter_input,
        FilterInput::Active {
            dimension: FilterDimension::Assignee,
            ..
        }
    ));

    // Typing appends to the buffer without filtering.
    for c in "founder".chars() {
        dispatch(&mut app, Action::FilterChar(c), &deps).unwrap();
    }
    assert!(matches!(
        &app.filter_input,
        FilterInput::Active { buffer, .. } if buffer == "founder"
    ));
    assert_eq!(app.model.len(), 2, "typing does not filter yet");

    // Enter applies: only the founder's card remains, reloaded.
    dispatch(&mut app, Action::FilterApply, &deps).unwrap();
    assert_eq!(app.filters.assignee.as_deref(), Some("founder"));
    assert_eq!(
        app.model.len(),
        1,
        "model reloaded with the assignee filter"
    );

    // `/` reopens the bar but never clears the applied filter.
    dispatch(&mut app, Action::FilterOpen, &deps).unwrap();
    assert_eq!(
        app.filters.assignee.as_deref(),
        Some("founder"),
        "reopening the bar does not clear the filter"
    );

    // Empty-Enter clears the active dimension.
    dispatch(&mut app, Action::FilterApply, &deps).unwrap();
    assert_eq!(
        app.filters.assignee, None,
        "empty enter clears the dimension"
    );
    assert_eq!(app.model.len(), 2, "cleared filter shows all cards");

    // `x` while the bar is open clears every filter and closes the bar.
    dispatch(&mut app, Action::FilterOpen, &deps).unwrap();
    for c in "founder".chars() {
        dispatch(&mut app, Action::FilterChar(c), &deps).unwrap();
    }
    dispatch(&mut app, Action::FilterApply, &deps).unwrap();
    assert_eq!(app.filters.assignee.as_deref(), Some("founder"));
    dispatch(&mut app, Action::ClearFilters, &deps).unwrap();
    assert_eq!(app.filters.assignee, None);
    assert!(!app.filter_input.is_active(), "x closes the bar");

    // Esc closes without applying a fresh buffer.
    dispatch(&mut app, Action::FilterOpen, &deps).unwrap();
    for c in "other".chars() {
        dispatch(&mut app, Action::FilterChar(c), &deps).unwrap();
    }
    dispatch(&mut app, Action::FilterClose, &deps).unwrap();
    assert!(!app.filter_input.is_active());
    assert_eq!(
        app.filters.assignee, None,
        "esc closes without applying the buffer"
    );

    // Tab cycles dimension (assignee → label).
    dispatch(&mut app, Action::FilterOpen, &deps).unwrap();
    dispatch(&mut app, Action::FilterNext, &deps).unwrap();
    assert!(matches!(
        &app.filter_input,
        FilterInput::Active {
            dimension: FilterDimension::Label,
            ..
        }
    ));
}
