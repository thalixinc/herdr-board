//! Draft creation lifecycle (G5): atomic factory-kind persistence, reopen
//! durability, the monotonic promote transition, and the inertness rule.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use herdr_board::{
    create_draft, factory_kind_of, promote_to_factory_request, CardError, FactoryKind, Store,
    SCHEMA_VERSION,
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
            "herdr-board-draft-{}-{nanos}-{n}",
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

#[test]
fn create_draft_persists_kind_atomically() {
    let store = Store::open_in_memory().expect("open store");
    let draft =
        create_draft(&store, FactoryKind::Ordinary, "Fix the board", "body").expect("create");
    assert_eq!(draft.factory_kind, FactoryKind::Ordinary);
    assert_eq!(draft.title, "Fix the board");
    assert!(draft.created_at > 0);
    assert!(draft.published_at.is_none());

    // The kind is in the same row — read straight back.
    assert_eq!(
        factory_kind_of(&store, &draft.draft_id).expect("read kind"),
        FactoryKind::Ordinary
    );
}

#[test]
fn create_factory_draft_directly() {
    let store = Store::open_in_memory().expect("open store");
    let draft = create_draft(
        &store,
        FactoryKind::FactoryRequest,
        "Process with factory",
        "body",
    )
    .expect("create");
    assert_eq!(draft.factory_kind, FactoryKind::FactoryRequest);
    assert_eq!(
        factory_kind_of(&store, &draft.draft_id).expect("read kind"),
        FactoryKind::FactoryRequest
    );
}

#[test]
fn factory_kind_survives_reopen() {
    let dir = TestDir::new();
    let path = dir.file();
    let draft_id;
    {
        let store = Store::open(&path).expect("open store");
        let draft = create_draft(
            &store,
            FactoryKind::FactoryRequest,
            "Process with factory",
            "body",
        )
        .expect("create");
        draft_id = draft.draft_id;
    } // drop closes the connection

    {
        let store = Store::open(&path).expect("reopen store");
        assert_eq!(
            factory_kind_of(&store, &draft_id).expect("read kind"),
            FactoryKind::FactoryRequest
        );
    }
}

#[test]
fn promote_preserves_factory_kind_and_builds_request() {
    let store = Store::open_in_memory().expect("open store");
    let draft =
        create_draft(&store, FactoryKind::Ordinary, "card", "request body").expect("create");

    let request = promote_to_factory_request(&store, &draft.draft_id).expect("promote");
    assert_eq!(request.factory_kind, FactoryKind::FactoryRequest);
    assert_eq!(request.schema_version, SCHEMA_VERSION);
    assert_eq!(request.body, "request body");

    // The draft's kind is now factory-request (monotonic transition).
    assert_eq!(
        factory_kind_of(&store, &draft.draft_id).expect("read kind"),
        FactoryKind::FactoryRequest
    );
}

#[test]
fn cannot_promote_twice() {
    let store = Store::open_in_memory().expect("open store");
    let draft = create_draft(&store, FactoryKind::Ordinary, "card", "body").expect("create");
    promote_to_factory_request(&store, &draft.draft_id).expect("first promote");

    assert!(matches!(
        promote_to_factory_request(&store, &draft.draft_id),
        Err(CardError::AlreadyPromoted)
    ));
}

#[test]
fn promote_rejects_already_factory_draft() {
    let store = Store::open_in_memory().expect("open store");
    let draft = create_draft(
        &store,
        FactoryKind::FactoryRequest,
        "Process with factory",
        "body",
    )
    .expect("create");

    assert!(matches!(
        promote_to_factory_request(&store, &draft.draft_id),
        Err(CardError::AlreadyPromoted)
    ));
}

#[test]
fn factory_kind_of_missing_draft_is_not_found() {
    let store = Store::open_in_memory().expect("open store");
    assert!(matches!(
        factory_kind_of(&store, "does-not-exist"),
        Err(CardError::NotFound)
    ));
    assert!(matches!(
        promote_to_factory_request(&store, "does-not-exist"),
        Err(CardError::NotFound)
    ));
}

#[test]
fn promote_does_not_start_work() {
    // sync-never-starts-work: promote builds a request record but never
    // persists it, dispatches it, or starts any handoff.
    let store = Store::open_in_memory().expect("open store");
    let draft = create_draft(&store, FactoryKind::Ordinary, "card", "body").expect("create");
    promote_to_factory_request(&store, &draft.draft_id).expect("promote");

    // No request was persisted, so no non-terminal receipt (attempt) exists.
    assert!(store.list_non_terminal().expect("list").is_empty());
}
