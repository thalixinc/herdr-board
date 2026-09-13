use std::env;
use std::io;

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use herdr_board::{
    run_loop, App, CfSubmission, Credentials, Deps, RealGitHubClient, RealHandoffTransport,
    RepoIdentity, Store,
};

/// Resolve the scoped repo from the environment, with a dev fallback.
///
/// Order: `HERDR_BOARD_REPO` (`owner/repo`), then a `repo` field inside
/// `HERDR_PLUGIN_CONTEXT_JSON`, then the dev constant.
fn resolve_repo() -> RepoIdentity {
    if let Ok(repo) = env::var("HERDR_BOARD_REPO") {
        if let Some(parsed) = RepoIdentity::parse(&repo) {
            return parsed;
        }
    }
    if let Ok(json) = env::var("HERDR_PLUGIN_CONTEXT_JSON") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
            if let Some(repo) = value.get("repo").and_then(|r| r.as_str()) {
                if let Some(parsed) = RepoIdentity::parse(repo) {
                    return parsed;
                }
            }
        }
    }
    RepoIdentity::new("thalixinc", "herdr-board")
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let repo = resolve_repo();
    let store = Store::open(Store::default_path()).map_err(io::Error::other)?;

    let client = RealGitHubClient::new(Credentials::from_env());
    // The concrete cf-queue wire invocation is a later integration; until then
    // the real transport reports the submission as unreachable (Failed).
    let transport =
        RealHandoffTransport::new(Credentials::from_env(), |_contract| CfSubmission::Failed {
            reason: "cf-queue wire invocation is a later integration".to_owned(),
        });

    let deps = Deps {
        pull: &client,
        github: &client,
        transport: &transport,
        repo: &repo,
    };

    let mut app = App::new(store, repo.clone());
    run_loop(terminal, &mut app, &deps)
}
