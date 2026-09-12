//! Create-intent outbox lifecycle: insert/transition, the guarded
//! `pending → issued` block-auto-replay transition, marker uniqueness,
//! `list_issued`, and restart persistence.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use herdr_board::outbox::{CreateIntent, CreateOutcome, Marker, Store, StoreError};
use herdr_board::FactoryKind;

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
            "herdr-board-intent-{}-{nanos}-{n}",
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

fn intent(marker: Marker) -> CreateIntent {
    CreateIntent::new(
        marker,
        "thalixinc/herdr-board".to_string(),
        "Fix the board sync".to_string(),
        "Detailed body".to_string(),
        "[\"bug\"]".to_string(),
        FactoryKind::Ordinary,
    )
}

#[test]
fn insert_and_transition_to_created() {
    let store = Store::open_in_memory().expect("open store");
    let marker = Marker::new_v4();
    let draft = intent(marker);

    let inserted = store.insert_intent(&draft).expect("insert intent");
    assert_eq!(inserted.intent_id, draft.intent_id);
    assert_eq!(inserted.marker, marker);
    assert_eq!(inserted.outcome, CreateOutcome::Pending);
    assert_eq!(inserted.title, "Fix the board sync");
    assert!(inserted.created_at > 0);
    assert!(inserted.issued_at.is_none());
    assert!(inserted.finalized_at.is_none());
    assert!(inserted.issue_number.is_none());
    assert!(inserted.reason.is_none());

    let issued = store.mark_issued(&draft.intent_id).expect("mark issued");
    assert_eq!(issued.outcome, CreateOutcome::Issued);
    assert!(issued.issued_at.is_some());
    assert!(issued.finalized_at.is_none());

    let created = store
        .mark_created(&draft.intent_id, 4242)
        .expect("mark created");
    assert_eq!(created.outcome, CreateOutcome::Created);
    assert_eq!(created.issue_number, Some(4242));
    assert!(created.finalized_at.is_some());
    assert!(created.outcome.is_terminal());
}

#[test]
fn mark_issued_is_guarded() {
    let store = Store::open_in_memory().expect("open store");
    let draft = intent(Marker::new_v4());
    store.insert_intent(&draft).expect("insert intent");
    store
        .mark_issued(&draft.intent_id)
        .expect("first mark_issued");

    // The create may be sent at most once: a second mark_issued is refused.
    assert!(matches!(
        store.mark_issued(&draft.intent_id),
        Err(StoreError::CreateIntentNotPending)
    ));
}

#[test]
fn duplicate_marker_is_rejected() {
    let store = Store::open_in_memory().expect("open store");
    let marker = Marker::new_v4();

    store
        .insert_intent(&intent(marker))
        .expect("first insert intent");

    // Same marker, different intent id → UNIQUE(marker) fires.
    assert!(matches!(
        store.insert_intent(&intent(marker)),
        Err(StoreError::DuplicateMarker)
    ));
}

#[test]
fn list_issued_returns_only_issued() {
    let store = Store::open_in_memory().expect("open store");
    let a = intent(Marker::new_v4());
    let b = intent(Marker::new_v4());
    let pending = intent(Marker::new_v4());

    store.insert_intent(&a).expect("insert a");
    store.insert_intent(&b).expect("insert b");
    store.insert_intent(&pending).expect("insert pending");

    store.mark_issued(&a.intent_id).expect("issue a");
    store.mark_issued(&b.intent_id).expect("issue b");

    let issued = store.list_issued().expect("list issued");
    let ids: Vec<_> = issued.iter().map(|i| i.intent_id).collect();
    assert_eq!(issued.len(), 2);
    assert!(ids.contains(&a.intent_id));
    assert!(ids.contains(&b.intent_id));
    assert!(!ids.contains(&pending.intent_id));
}

#[test]
fn mark_failed_stores_reason() {
    let store = Store::open_in_memory().expect("open store");
    let draft = intent(Marker::new_v4());
    store.insert_intent(&draft).expect("insert intent");
    store.mark_issued(&draft.intent_id).expect("mark issued");

    let failed = store
        .mark_failed(&draft.intent_id, "validation: missing labels")
        .expect("mark failed");
    assert_eq!(failed.outcome, CreateOutcome::Failed);
    assert_eq!(failed.reason.as_deref(), Some("validation: missing labels"));
    assert!(failed.finalized_at.is_some());
    assert!(failed.issue_number.is_none());
}

#[test]
fn mark_cancelled_guarded_to_non_terminal() {
    let store = Store::open_in_memory().expect("open store");

    // Cancel from pending is allowed.
    let a = intent(Marker::new_v4());
    store.insert_intent(&a).expect("insert a");
    let cancelled = store.mark_cancelled(&a.intent_id).expect("cancel pending");
    assert_eq!(cancelled.outcome, CreateOutcome::Cancelled);
    assert!(cancelled.finalized_at.is_some());

    // Cancel from issued is allowed.
    let b = intent(Marker::new_v4());
    store.insert_intent(&b).expect("insert b");
    store.mark_issued(&b.intent_id).expect("issue b");
    let cancelled = store.mark_cancelled(&b.intent_id).expect("cancel issued");
    assert_eq!(cancelled.outcome, CreateOutcome::Cancelled);

    // Cancel from a terminal intent is refused.
    assert!(matches!(
        store.mark_cancelled(&a.intent_id),
        Err(StoreError::CreateIntentNotPending)
    ));
}

#[test]
fn persists_across_reopen() {
    let dir = TestDir::new();
    let path = dir.file();
    let marker = Marker::new_v4();
    let intent_id;

    {
        let store = Store::open(&path).expect("open store");
        let draft = intent(marker);
        intent_id = draft.intent_id;
        store.insert_intent(&draft).expect("insert intent");
        store.mark_issued(&draft.intent_id).expect("mark issued");
    } // drop closes the connection

    // Reopen the same file: migration v2 is idempotent and the issued row
    // survives.
    {
        let store = Store::open(&path).expect("reopen store");
        let fetched = store
            .get_by_marker(&marker)
            .expect("get by marker")
            .expect("intent persists");
        assert_eq!(fetched.intent_id, intent_id);
        assert_eq!(fetched.outcome, CreateOutcome::Issued);

        let issued = store.list_issued().expect("list issued");
        assert_eq!(issued.len(), 1);
        assert_eq!(issued[0].intent_id, intent_id);
    }
}
