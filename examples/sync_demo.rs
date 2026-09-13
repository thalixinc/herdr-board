//! Runnable smoke: sync → re-sync with a changed body → apply.

use herdr_board::{apply_changes, sync, Identity, IssueFull, PullClient, RepoIdentity, Store};

struct FakeClient {
    issues: Vec<IssueFull>,
}

impl PullClient for FakeClient {
    fn list_issues(&self, _repo: &RepoIdentity) -> Vec<IssueFull> {
        self.issues.clone()
    }
    fn fetch_issue(&self, _repo: &RepoIdentity, number: u64) -> Option<IssueFull> {
        self.issues.iter().find(|i| i.number == number).cloned()
    }
}

fn issue(number: u64, title: &str, body: &str, updated_at: &str) -> IssueFull {
    IssueFull {
        number,
        title: title.to_owned(),
        body: body.to_owned(),
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
        "herdr-board-sync-demo-{}-{}",
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

    // 1. First pull: inserts.
    let client = FakeClient {
        issues: vec![issue(1, "Fix sync", "pull issues", "2026-09-12T00:00:00Z")],
    };
    let summary = sync(&store, &client, &repo).unwrap();
    println!(
        "sync: inserted={} unchanged={} conflicted={}",
        summary.inserted, summary.unchanged, summary.conflicted
    );

    // 2. GitHub changes the body; re-sync surfaces a conflict.
    let client = FakeClient {
        issues: vec![issue(
            1,
            "Fix sync",
            "pull issues (revised)",
            "2026-09-12T01:00:00Z",
        )],
    };
    let summary = sync(&store, &client, &repo).unwrap();
    println!(
        "re-sync: inserted={} unchanged={} conflicted={}",
        summary.inserted, summary.unchanged, summary.conflicted
    );

    let identity = Identity::new("thalixinc", "herdr-board", 1);
    let card = store.get_card(&identity).unwrap().unwrap();
    println!("card conflict: {}", card.conflict.as_str());
    for diff in store.get_conflict(&identity).unwrap() {
        println!(
            "  {}: {:?} -> {:?}",
            diff.field.as_str(),
            diff.old,
            diff.new
        );
    }

    // 3. Apply the change: clears the conflict and adopts the new body.
    let applied = apply_changes(&store, &client, &repo, 1).unwrap();
    println!(
        "applied: conflict={} body={:?}",
        applied.conflict.as_str(),
        applied.fields.body
    );

    let _ = std::fs::remove_dir_all(&dir);
}
