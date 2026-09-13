//! Integration: GitHub issue pull sync against a fake `PullClient`.

use herdr_board::{
    apply_changes, defer_changes, sync, CanonicalFields, Card, CardField, Conflict, FactoryKind,
    Identity, IssueFull, PullClient, RepoIdentity, Store, DEFAULT_COLUMN,
};

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

fn repo() -> RepoIdentity {
    RepoIdentity::new("ThalixInc", "herdr-board")
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

fn identity(number: u64) -> Identity {
    Identity::new("thalixinc", "herdr-board", number)
}

#[test]
fn first_pull_inserts_with_defaults() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient {
        issues: vec![issue(1, "Title", "Body", "2026-09-12T00:00:00Z")],
    };
    let summary = sync(&store, &client, &repo()).unwrap();
    assert_eq!(summary.inserted, 1);
    assert_eq!(summary.unchanged, 0);
    assert_eq!(summary.conflicted, 0);

    let card = store.get_card(&identity(1)).unwrap().unwrap();
    assert_eq!(card.column, DEFAULT_COLUMN);
    assert_eq!(card.factory_kind, FactoryKind::Ordinary);
    assert_eq!(card.conflict, Conflict::None);
    assert_eq!(card.fields.title, "Title");
    assert_eq!(card.fields.body, "Body");
    assert_eq!(card.revision, "2026-09-12T00:00:00Z");
    assert_eq!(
        card.url,
        "https://github.com/thalixinc/herdr-board/issues/1"
    );
}

#[test]
fn unchanged_resync_is_a_noop() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient {
        issues: vec![issue(1, "Title", "Body", "2026-09-12T00:00:00Z")],
    };
    let first = sync(&store, &client, &repo()).unwrap();
    assert_eq!(first.inserted, 1);

    let second = sync(&store, &client, &repo()).unwrap();
    assert_eq!(second.inserted, 0);
    assert_eq!(second.unchanged, 1);
    assert_eq!(second.conflicted, 0);

    let card = store.get_card(&identity(1)).unwrap().unwrap();
    assert_eq!(card.conflict, Conflict::None);
    assert_eq!(card.fields.body, "Body");
}

#[test]
fn changed_updated_at_flags_conflict_with_exact_diffs() {
    let store = Store::open_in_memory().unwrap();
    let repo = repo();

    sync(
        &store,
        &FakeClient {
            issues: vec![issue(1, "Title", "Body", "2026-09-12T00:00:00Z")],
        },
        &repo,
    )
    .unwrap();

    // Body and state drift; title/labels/assignee/milestone do not.
    let mut changed = issue(1, "Title", "Body (edited)", "2026-09-12T01:00:00Z");
    changed.state = "closed".to_owned();
    let summary = sync(
        &store,
        &FakeClient {
            issues: vec![changed],
        },
        &repo,
    )
    .unwrap();
    assert_eq!(summary.conflicted, 1);
    assert_eq!(summary.inserted, 0);
    assert_eq!(summary.unchanged, 0);

    let card = store.get_card(&identity(1)).unwrap().unwrap();
    assert_eq!(card.conflict, Conflict::ApplyPending);
    // The board copy is NOT silently overwritten.
    assert_eq!(card.fields.body, "Body");
    assert_eq!(card.fields.state, "open");

    let diffs = store.get_conflict(&identity(1)).unwrap();
    let fields: Vec<CardField> = diffs.iter().map(|d| d.field).collect();
    assert_eq!(fields, vec![CardField::Body, CardField::State]);
    assert_eq!(diffs[0].old, "Body");
    assert_eq!(diffs[0].new, "Body (edited)");
}

#[test]
fn apply_accepts_only_the_seven_shared_fields() {
    let store = Store::open_in_memory().unwrap();
    let repo = repo();

    // Seed a board-local card with non-default column/factory_kind.
    store
        .insert_card(&Card {
            identity: identity(1),
            url: "https://github.com/thalixinc/herdr-board/issues/1".to_owned(),
            fields: CanonicalFields {
                title: "Old title".to_owned(),
                body: "Old body".to_owned(),
                state: "open".to_owned(),
                state_reason: None,
                labels: vec![],
                assignee: None,
                milestone: None,
            },
            column: "doing".to_owned(),
            factory_kind: FactoryKind::FactoryRequest,
            revision: "r1".to_owned(),
            conflict: Conflict::None,
            synced_at: 0,
        })
        .unwrap();

    let mut changed = issue(1, "New title", "New body", "r2");
    changed.labels = vec!["bug".to_owned()];
    changed.assignee = Some("alice".to_owned());
    let summary = sync(
        &store,
        &FakeClient {
            issues: vec![changed.clone()],
        },
        &repo,
    )
    .unwrap();
    assert_eq!(summary.conflicted, 1);

    let applied = apply_changes(
        &store,
        &FakeClient {
            issues: vec![changed],
        },
        &repo,
        1,
    )
    .unwrap();
    assert_eq!(applied.conflict, Conflict::None);
    assert_eq!(applied.fields.title, "New title");
    assert_eq!(applied.fields.body, "New body");
    assert_eq!(applied.fields.labels, vec!["bug".to_owned()]);
    assert_eq!(applied.fields.assignee.as_deref(), Some("alice"));
    assert_eq!(applied.revision, "r2");
    // Board-local fields survive apply.
    assert_eq!(applied.column, "doing");
    assert_eq!(applied.factory_kind, FactoryKind::FactoryRequest);
    assert_eq!(applied.synced_at, 0);
}

#[test]
fn defer_keeps_the_flag() {
    let store = Store::open_in_memory().unwrap();
    let repo = repo();

    sync(
        &store,
        &FakeClient {
            issues: vec![issue(1, "Title", "Body", "r1")],
        },
        &repo,
    )
    .unwrap();
    sync(
        &store,
        &FakeClient {
            issues: vec![issue(1, "Title", "Body (edited)", "r2")],
        },
        &repo,
    )
    .unwrap();

    defer_changes(&store, &identity(1)).unwrap();
    let card = store.get_card(&identity(1)).unwrap().unwrap();
    assert_eq!(card.conflict, Conflict::ApplyPending);
    // The diff rows are retained.
    assert_eq!(store.get_conflict(&identity(1)).unwrap().len(), 1);
}

#[test]
fn empty_labels_assignee_milestone_tolerated() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient {
        issues: vec![issue(1, "Title", "Body", "r1")],
    };
    let summary = sync(&store, &client, &repo()).unwrap();
    assert_eq!(summary.inserted, 1);

    let card = store.get_card(&identity(1)).unwrap().unwrap();
    assert!(card.fields.labels.is_empty());
    assert!(card.fields.assignee.is_none());
    assert!(card.fields.milestone.is_none());
    assert!(card.fields.state_reason.is_none());
}
