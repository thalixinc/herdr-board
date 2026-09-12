//! Outbox store lifecycle: CRUD, dedup, active-attempt uniqueness, and
//! restart persistence — exercised purely against SQLite (in-memory + a real
//! temp file), never the environment.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use herdr_board::digest::Digest;
use herdr_board::outbox::{HandoffId, Outcome, ReceiptId, RequestRecord, Store, StoreError};

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
            "herdr-board-outbox-{}-{nanos}-{n}",
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

fn request(tag: u8) -> RequestRecord {
    RequestRecord {
        digest: Digest([tag; 32]),
        identity: format!("owner/repo#{tag}"),
        revision: "r1".to_string(),
        factory: "coordinator".to_string(),
        actor: "founder".to_string(),
        actor_source: "herdr-workspace-identity".to_string(),
        body: format!("body-{tag}"),
    }
}

#[test]
fn insert_read_transition() {
    let store = Store::open_in_memory().expect("open in-memory store");
    let handoff = HandoffId::new_v4();
    let req = request(1);

    let receipt = store
        .insert_pending(&req, &handoff)
        .expect("insert pending");
    assert_eq!(receipt.outcome, Outcome::Pending);
    assert_eq!(receipt.handoff_id, handoff);
    assert_eq!(receipt.digest, req.digest);
    assert_eq!(receipt.digest_id, req.digest.digest_id());
    assert_eq!(receipt.identity, "owner/repo#1");
    assert_eq!(receipt.actor, "founder");
    assert_eq!(receipt.actor_source, "herdr-workspace-identity");
    assert!(receipt.created_at > 0);
    assert!(receipt.finalized_at.is_none());

    // Read back by handoff id.
    let fetched = store
        .get_by_handoff_id(&handoff)
        .expect("query by handoff")
        .expect("receipt exists");
    assert_eq!(fetched.receipt_id, receipt.receipt_id);
    assert_eq!(fetched.outcome, Outcome::Pending);

    // pending -> handed-off (non-terminal; no finalize).
    let handed = store
        .transition_to(&receipt.receipt_id, Outcome::HandedOff, None)
        .expect("transition to handed-off");
    assert_eq!(handed.outcome, Outcome::HandedOff);
    assert!(handed.finalized_at.is_none());

    // handed-off -> accepted (terminal, with external response).
    let accepted = store
        .transition_to(&receipt.receipt_id, Outcome::Accepted, Some("ack-ok"))
        .expect("finalize accepted");
    assert_eq!(accepted.outcome, Outcome::Accepted);
    assert!(accepted.finalized_at.is_some());
    assert_eq!(accepted.external_response.as_deref(), Some("ack-ok"));

    // Terminal is final: no further transition.
    assert!(matches!(
        store.transition_to(&receipt.receipt_id, Outcome::Cancelled, None),
        Err(StoreError::AlreadyFinal)
    ));

    // Unknown receipt id -> NotFound.
    assert!(matches!(
        store.transition_to(&ReceiptId::new_v4(), Outcome::Accepted, None),
        Err(StoreError::NotFound)
    ));

    // Unknown handoff id -> None.
    assert!(store
        .get_by_handoff_id(&HandoffId::new_v4())
        .expect("query unknown handoff")
        .is_none());
}

#[test]
fn dedup_rejects_duplicate_inflight_digest() {
    let store = Store::open_in_memory().expect("open in-memory store");
    let req = request(7);

    let first = store
        .insert_pending(&req, &HandoffId::new_v4())
        .expect("first insert");

    // Same input (same digest), new handoff, while in-flight -> rejected.
    assert!(matches!(
        store.insert_pending(&req, &HandoffId::new_v4()),
        Err(StoreError::ActiveAttemptExists)
    ));

    // The active attempt is still the first receipt.
    let active = store
        .get_active_by_digest(&req.digest)
        .expect("query active by digest")
        .expect("active receipt exists");
    assert_eq!(active.receipt_id, first.receipt_id);
    assert_eq!(active.handoff_id, first.handoff_id);
}

#[test]
fn active_attempt_rejects_second_inflight_for_request() {
    let store = Store::open_in_memory().expect("open in-memory store");
    let req = request(9);

    let first = store
        .insert_pending(&req, &HandoffId::new_v4())
        .expect("first insert");

    // The active attempt is queryable by request id.
    let active = store
        .get_active_by_request(&first.request_id)
        .expect("query active by request")
        .expect("active receipt exists");
    assert_eq!(active.receipt_id, first.receipt_id);

    // A second in-flight attempt for the same request is rejected.
    assert!(matches!(
        store.insert_pending(&req, &HandoffId::new_v4()),
        Err(StoreError::ActiveAttemptExists)
    ));

    // Exactly one non-terminal receipt exists.
    let non_terminal = store.list_non_terminal().expect("list non-terminal");
    assert_eq!(non_terminal.len(), 1);
    assert_eq!(non_terminal[0].receipt_id, first.receipt_id);
}

#[test]
fn terminal_outcome_allows_fresh_attempt_of_same_input() {
    // The uniqueness is scoped to non-terminal outcomes: a global UNIQUE(digest)
    // would wrongly block this, but a terminal receipt must not.
    let store = Store::open_in_memory().expect("open in-memory store");
    let req = request(3);

    let first = store
        .insert_pending(&req, &HandoffId::new_v4())
        .expect("first insert");
    store
        .transition_to(&first.receipt_id, Outcome::Accepted, Some("ok"))
        .expect("finalize first");

    // Same input after a terminal outcome is a fresh attempt: allowed.
    let second = store
        .insert_pending(&req, &HandoffId::new_v4())
        .expect("fresh attempt allowed after terminal");
    assert_ne!(second.receipt_id, first.receipt_id);
    assert_eq!(
        second.request_id, first.request_id,
        "same request row reused"
    );
    assert_eq!(second.outcome, Outcome::Pending);
}

#[test]
fn duplicate_handoff_id_is_rejected() {
    let store = Store::open_in_memory().expect("open in-memory store");
    let req = request(2);
    let handoff = HandoffId::new_v4();

    let first = store.insert_pending(&req, &handoff).expect("first insert");

    // Finalize, so the only violated constraint on re-delivery is the
    // handoff-id uniqueness (the active-attempt slot is now free).
    store
        .transition_to(&first.receipt_id, Outcome::Accepted, Some("ok"))
        .expect("finalize first");

    // Re-delivering the same handoff id -> duplicate handoff.
    assert!(matches!(
        store.insert_pending(&req, &handoff),
        Err(StoreError::DuplicateHandoff)
    ));

    // The stored receipt is unchanged (idempotent re-delivery is a no-op).
    let fetched = store
        .get_by_handoff_id(&handoff)
        .expect("query by handoff")
        .expect("receipt exists");
    assert_eq!(fetched.receipt_id, first.receipt_id);
    assert_eq!(fetched.outcome, Outcome::Accepted);
}

#[test]
fn persists_across_reopen() {
    let dir = TestDir::new();
    let path = dir.file();
    let req = request(11);
    let handoff = HandoffId::new_v4();

    let receipt_id;
    {
        let store = Store::open(&path).expect("open store");
        let receipt = store
            .insert_pending(&req, &handoff)
            .expect("insert pending");
        receipt_id = receipt.receipt_id;
    } // drop closes the connection

    // Reopen the same file; WAL-committed rows survive.
    {
        let store = Store::open(&path).expect("reopen store");
        let fetched = store
            .get_by_handoff_id(&handoff)
            .expect("query by handoff")
            .expect("receipt persists across reopen");
        assert_eq!(fetched.receipt_id, receipt_id);
        assert_eq!(fetched.outcome, Outcome::Pending);

        let non_terminal = store.list_non_terminal().expect("list non-terminal");
        assert_eq!(non_terminal.len(), 1);
        assert_eq!(non_terminal[0].receipt_id, receipt_id);
    }
}

#[test]
fn default_path_uses_state_dir_when_set() {
    // Path resolution must not touch the filesystem; only assert the env is
    // honored by pointing HERDR_PLUGIN_STATE_DIR at a temp dir.
    let dir = TestDir::new();
    let prior = std::env::var_os("HERDR_PLUGIN_STATE_DIR");
    std::env::set_var(
        "HERDR_PLUGIN_STATE_DIR",
        dir.0.to_str().expect("utf-8 path"),
    );

    let path = Store::default_path();
    assert_eq!(path, dir.0.join("herdr-board.sqlite3"));

    match prior {
        Some(v) => std::env::set_var("HERDR_PLUGIN_STATE_DIR", v),
        None => std::env::remove_var("HERDR_PLUGIN_STATE_DIR"),
    }
}
