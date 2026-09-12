//! Runnable smoke of the create path: draft → issue (lost response) → startup
//! sweep → manual link, against a tempdir SQLite file and a fake client.

use std::path::PathBuf;

use herdr_board::{
    issue, link, Candidate, CreateIntent, CreateOutcome, CreateResult, FactoryKind, GitHubClient,
    Issue, Marker, RepoIdentity, Store,
};

/// Always loses the response (uncertain), but knows issue 1234 exists for the
/// manual-link step.
struct LostResponseClient;

impl GitHubClient for LostResponseClient {
    fn create_issue(
        &self,
        _repo: &RepoIdentity,
        _intent: &CreateIntent,
        _marker_comment: &str,
    ) -> CreateResult {
        CreateResult::Uncertain("network drop".to_string())
    }

    fn search_issues(&self, _repo: &RepoIdentity, _query: &str) -> Vec<Candidate> {
        Vec::new()
    }

    fn get_issue(&self, _repo: &RepoIdentity, number: u64) -> Option<Issue> {
        (number == 1234).then(|| Issue {
            number,
            title: "Fix the board sync".to_string(),
        })
    }
}

fn main() {
    let dir = std::env::temp_dir().join(format!("herdr-board-create-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path: PathBuf = dir.join("demo.sqlite3");

    let store = Store::open(&path).expect("open store");
    let client = LostResponseClient;

    let intent = CreateIntent::new(
        Marker::new_v4(),
        "thalixinc/herdr-board".to_string(),
        "Fix the board sync".to_string(),
        "The board should sync issues".to_string(),
        "[\"bug\"]".to_string(),
        FactoryKind::Ordinary,
    );

    // Draft → issue: the response is lost, so the create is uncertain.
    match issue(&store, &client, &intent) {
        Err(e) => println!("issue: {e}"),
        Ok(_) => {
            eprintln!("expected uncertain create");
            std::process::exit(1);
        }
    }

    // The intent now sits in `issued` (outcome unknown) and surfaces in the sweep.
    let swept = herdr_board::create::startup_sweep(&store).expect("sweep");
    for i in &swept {
        println!("intent {} outcome {}", i.intent_id, i.outcome.as_str());
    }

    // Human links it to the real issue (number 1234), never re-creating.
    let linked = link(&store, &client, &intent.intent_id, 1234).expect("link");
    assert_eq!(linked, CreateOutcome::Created);
    println!("linked issue 1234");

    let _ = std::fs::remove_dir_all(&dir);
}
