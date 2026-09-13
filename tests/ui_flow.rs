//! Integration: the full sync → render → move → render cycle against a fake
//! client and a tempdir store.

use std::cell::RefCell;

use herdr_board::{
    dispatch, render, sync, Action, App, Candidate, CanonicalRequest, CreateIntent, CreateResult,
    Deps, ExternalResponse, GitHubClient, HandoffResult, HandoffTransport, Issue, IssueFull,
    IssuePatch, PullClient, RepoIdentity, Store, UpdateResult,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

struct FakeClient {
    issues: RefCell<Vec<IssueFull>>,
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
        _intent: &CreateIntent,
        _marker: &str,
    ) -> CreateResult {
        CreateResult::Failed("unused".to_owned())
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
        UpdateResult::Failed("unused".to_owned())
    }
}

struct FakeTransport;

impl HandoffTransport for FakeTransport {
    fn handoff(&self, _request: &CanonicalRequest) -> HandoffResult {
        HandoffResult::Accepted(ExternalResponse::new("ok"))
    }
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

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "herdr-board-ui-flow-{}-{}",
        std::process::id(),
        herdr_board::HandoffId::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn render_text(app: &App) -> String {
    let backend = TestBackend::new(160, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let area = *buffer.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            out.push_str(buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
        }
        out.push('\n');
    }
    out
}

#[test]
fn sync_render_move_render_cycle() {
    let dir = temp_dir();
    let store = Store::open(dir.join("herdr-board.sqlite3")).unwrap();
    let repo = RepoIdentity::new("ThalixInc", "herdr-board");
    let client = FakeClient {
        issues: RefCell::new(vec![issue(1, "Fix sync", "r1")]),
    };
    let transport = FakeTransport;

    // 1. Sync inserts the card.
    let summary = sync(&store, &client, &repo).unwrap();
    assert_eq!(summary.inserted, 1);

    let deps = Deps {
        pull: &client,
        github: &client,
        transport: &transport,
        repo: &repo,
    };
    let mut app = App::new(store, repo.clone());

    // 2. Render shows the card in the first (default) column.
    let first = render_text(&app);
    assert!(first.contains("TO-DO"));
    assert!(first.contains("Fix sync"));

    // 3. Move right: board-local column change, then render again.
    dispatch(&mut app, Action::MoveRight, &deps).unwrap();
    assert_eq!(
        app.store
            .get_card(&herdr_board::Identity::new("thalixinc", "herdr-board", 1))
            .unwrap()
            .unwrap()
            .column,
        "in-progress"
    );
    let second = render_text(&app);
    assert!(second.contains("IN-PROGRESS"));
    assert!(second.contains("Fix sync"));

    let _ = std::fs::remove_dir_all(&dir);
}
