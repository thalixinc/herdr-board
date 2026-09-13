//! Card store lifecycle (VS1): CRUD, idempotent identity insert, conflict
//! record/get/apply/defer, label round-trip, and restart persistence.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use herdr_board::{
    CanonicalFields, Card, CardField, CardFieldDiff, Conflict, FactoryKind, Identity, Store,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temp directory, removed on drop.
struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "herdr-board-cards-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TestDir(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("db.sqlite3")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn identity(number: u64) -> Identity {
    Identity::new("thalixinc", "herdr-board", number)
}

fn fields(title: &str, body: &str) -> CanonicalFields {
    CanonicalFields {
        title: title.to_string(),
        body: body.to_string(),
        state: "open".to_string(),
        state_reason: None,
        labels: vec!["bug".to_string(), "cf:hold".to_string()],
        assignee: Some("founder".to_string()),
        milestone: None,
    }
}

fn card(number: u64, title: &str, body: &str) -> Card {
    Card {
        identity: identity(number),
        url: format!("https://github.com/thalixinc/herdr-board/issues/{number}"),
        fields: fields(title, body),
        column: "to-do".to_string(),
        factory_kind: FactoryKind::Ordinary,
        revision: "2026-09-12T00:00:00Z".to_string(),
        conflict: Conflict::None,
        synced_at: 1000,
    }
}

#[test]
fn insert_get_list() {
    let store = Store::open_in_memory().expect("open store");
    store
        .insert_card(&card(1, "title a", "body a"))
        .expect("insert 1");
    store
        .insert_card(&card(2, "title b", "body b"))
        .expect("insert 2");

    let got = store
        .get_card(&identity(1))
        .expect("get")
        .expect("card 1 exists");
    assert_eq!(got.identity, identity(1));
    assert_eq!(got.fields.title, "title a");
    assert_eq!(
        got.fields.labels,
        vec!["bug".to_string(), "cf:hold".to_string()]
    );
    assert_eq!(got.column, "to-do");
    assert_eq!(got.factory_kind, FactoryKind::Ordinary);
    assert_eq!(got.conflict, Conflict::None);
    assert_eq!(got.revision, "2026-09-12T00:00:00Z");

    assert!(store.get_card(&identity(999)).expect("get").is_none());

    let listed = store.list_cards("thalixinc", "herdr-board").expect("list");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].identity.number, 1);
    assert_eq!(listed[1].identity.number, 2);

    // Scoped to a different repo → empty.
    assert!(store.list_cards("other", "repo").expect("list").is_empty());
}

#[test]
fn duplicate_identity_is_rejected() {
    let store = Store::open_in_memory().expect("open store");
    store
        .insert_card(&card(1, "title a", "body a"))
        .expect("insert");

    // Same identity (owner/repo/number) → unique-constraint, no second row.
    assert!(store.insert_card(&card(1, "different", "body")).is_err());
    assert_eq!(
        store
            .list_cards("thalixinc", "herdr-board")
            .expect("list")
            .len(),
        1
    );
}

#[test]
fn conflict_record_get_apply_defer() {
    let store = Store::open_in_memory().expect("open store");
    store
        .insert_card(&card(1, "old title", "old body"))
        .expect("insert");

    let diffs = vec![
        CardFieldDiff {
            field: CardField::Title,
            old: "old title".to_string(),
            new: "new title".to_string(),
        },
        CardFieldDiff {
            field: CardField::Body,
            old: "old body".to_string(),
            new: "new body".to_string(),
        },
    ];
    store
        .record_conflict(&identity(1), &diffs, 2000)
        .expect("record");

    // Flagged + diffs stored.
    assert_eq!(
        store
            .get_card(&identity(1))
            .expect("get")
            .expect("exists")
            .conflict,
        Conflict::ApplyPending
    );
    assert_eq!(store.get_conflict(&identity(1)).expect("get"), diffs);

    // Defer keeps the flag and the diffs.
    store.defer_conflict(&identity(1)).expect("defer");
    assert_eq!(
        store
            .get_card(&identity(1))
            .expect("get")
            .expect("exists")
            .conflict,
        Conflict::ApplyPending
    );
    assert_eq!(store.get_conflict(&identity(1)).expect("get").len(), 2);

    // Apply accepts the canonical fields, sets revision, clears conflict —
    // and leaves board-local column/factory_kind untouched.
    let new_fields = fields("new title", "new body");
    let updated = store
        .apply_conflict(&identity(1), &new_fields, "2026-09-12T01:00:00Z")
        .expect("apply");
    assert_eq!(updated.fields.title, "new title");
    assert_eq!(updated.fields.body, "new body");
    assert_eq!(updated.revision, "2026-09-12T01:00:00Z");
    assert_eq!(updated.conflict, Conflict::None);
    assert_eq!(updated.column, "to-do");
    assert_eq!(updated.factory_kind, FactoryKind::Ordinary);
    assert!(store.get_conflict(&identity(1)).expect("get").is_empty());
}

#[test]
fn apply_preserves_board_local_fields() {
    let store = Store::open_in_memory().expect("open store");
    let mut c = card(1, "t", "b");
    c.column = "in-progress".to_string();
    c.factory_kind = FactoryKind::FactoryRequest;
    store.insert_card(&c).expect("insert");

    store
        .record_conflict(
            &identity(1),
            &[CardFieldDiff {
                field: CardField::Body,
                old: "b".to_string(),
                new: "b2".to_string(),
            }],
            2000,
        )
        .expect("record");

    let updated = store
        .apply_conflict(&identity(1), &fields("t", "b2"), "rev2")
        .expect("apply");
    assert_eq!(updated.column, "in-progress", "column is board-local");
    assert_eq!(updated.factory_kind, FactoryKind::FactoryRequest);
    assert_eq!(updated.conflict, Conflict::None);
}

#[test]
fn labels_round_trip_through_store() {
    let store = Store::open_in_memory().expect("open store");
    let mut c = card(1, "t", "b");
    c.fields.labels = vec![
        "with space".to_string(),
        "comma,name".to_string(),
        "quote\"here".to_string(),
        "héllo".to_string(),
    ];
    store.insert_card(&c).expect("insert");

    let got = store.get_card(&identity(1)).expect("get").expect("exists");
    assert_eq!(got.fields.labels, c.fields.labels);
}

#[test]
fn empty_and_nullable_fields_round_trip() {
    let store = Store::open_in_memory().expect("open store");
    let mut c = card(1, "t", "b");
    c.fields.labels = vec![];
    c.fields.assignee = None;
    c.fields.milestone = None;
    c.fields.state_reason = None;
    store.insert_card(&c).expect("insert");

    let got = store.get_card(&identity(1)).expect("get").expect("exists");
    assert!(got.fields.labels.is_empty());
    assert!(got.fields.assignee.is_none());
    assert!(got.fields.milestone.is_none());
    assert!(got.fields.state_reason.is_none());
}

#[test]
fn persists_across_reopen() {
    let dir = TestDir::new();
    let path = dir.file();
    {
        let store = Store::open(&path).expect("open store");
        store.insert_card(&card(1, "t", "b")).expect("insert");
        store
            .record_conflict(
                &identity(1),
                &[CardFieldDiff {
                    field: CardField::State,
                    old: "open".to_string(),
                    new: "closed".to_string(),
                }],
                2000,
            )
            .expect("record");
    } // drop closes the connection

    {
        let store = Store::open(&path).expect("reopen store");
        let got = store.get_card(&identity(1)).expect("get").expect("exists");
        assert_eq!(got.conflict, Conflict::ApplyPending);
        assert_eq!(got.fields.title, "t");
        let diffs = store.get_conflict(&identity(1)).expect("get");
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].field, CardField::State);
    }
}
