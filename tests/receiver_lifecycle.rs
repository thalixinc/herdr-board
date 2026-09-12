//! Receiver lifecycle: clean handoff, idempotent re-delivery, in-flight dedup,
//! forged-actor refusal, drift refusal, restart persistence, and convergence.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use herdr_board::digest::{compute, CanonicalRequest, Identity};
use herdr_board::outbox::{HandoffId, Outcome, RequestRecord, Store};
use herdr_board::receiver::{
    receive, startup_sweep, ExternalResponse, Handoff, HandoffResult, HandoffTransport,
    ReceiverError,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const WORKSPACE: &str = "test-workspace";

fn set_test_workspace() {
    std::env::set_var("HERDR_WORKSPACE_ID", WORKSPACE);
}

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
            "herdr-board-receiver-{}-{nanos}-{n}",
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

fn request(body: &str) -> CanonicalRequest {
    CanonicalRequest {
        identity: Identity::new("thalixinc", "herdr-board", 42),
        revision: "r1".to_string(),
        factory: "coordinator".to_string(),
        actor: WORKSPACE.to_string(),
        body: body.to_string(),
    }
}

fn record_from(req: &CanonicalRequest) -> RequestRecord {
    RequestRecord {
        digest: compute(req),
        identity: req.identity.canonical(),
        revision: req.revision.clone(),
        factory: req.factory.clone(),
        actor: req.actor.clone(),
        actor_source: "herdr-workspace-identity".to_string(),
        body: req.body.clone(),
    }
}

fn handoff(id: HandoffId, body: &str) -> Handoff {
    Handoff {
        handoff_id: id,
        request: request(body),
    }
}

struct FakeTransport {
    accepted: bool,
}

impl HandoffTransport for FakeTransport {
    fn handoff(&self, _handoff: &Handoff) -> HandoffResult {
        HandoffResult {
            accepted: self.accepted,
            response: ExternalResponse(if self.accepted {
                "ack-ok".to_string()
            } else {
                "nack".to_string()
            }),
        }
    }
}

#[test]
fn clean_path_returns_accepted_receipt() {
    set_test_workspace();
    let store = Store::open_in_memory().expect("open store");
    let transport = FakeTransport { accepted: true };
    let h = HandoffId::new_v4();

    let receipt = receive(&store, &handoff(h, "body-1"), &transport).expect("receive ok");

    assert_eq!(receipt.handoff_id, h);
    assert_eq!(receipt.outcome, Outcome::Accepted);
    assert_eq!(receipt.external_response.as_deref(), Some("ack-ok"));
    assert!(receipt.finalized_at.is_some());
    assert_eq!(receipt.actor, WORKSPACE);
    assert_eq!(receipt.actor_source, "herdr-workspace-identity");
}

#[test]
fn idempotent_redelivery_returns_same_receipt() {
    set_test_workspace();
    let store = Store::open_in_memory().expect("open store");
    let transport = FakeTransport { accepted: true };
    let h = HandoffId::new_v4();

    let first = receive(&store, &handoff(h, "body-1"), &transport).expect("first receive");
    let second = receive(&store, &handoff(h, "body-1"), &transport).expect("re-delivery");

    assert_eq!(first.receipt_id, second.receipt_id);
    assert_eq!(second.outcome, Outcome::Accepted);
}

#[test]
fn inflight_dedup_returns_active_receipt() {
    set_test_workspace();
    let store = Store::open_in_memory().expect("open store");
    let transport = FakeTransport { accepted: true };

    // Prime an in-flight (handed-off) request directly on the store.
    let req = request("body-1");
    let record = record_from(&req);
    let h1 = HandoffId::new_v4();
    let r1 = store
        .insert_pending(&record, &h1)
        .expect("insert in-flight");
    store
        .transition_to(&r1.receipt_id, Outcome::HandedOff, None)
        .expect("mark handed-off");

    // A new handoff id with the same input converges on the active receipt.
    let h2 = HandoffId::new_v4();
    let result = receive(&store, &handoff(h2, "body-1"), &transport).expect("receive ok");

    assert_eq!(result.receipt_id, r1.receipt_id);
    assert_eq!(result.handoff_id, h1);
    assert_eq!(result.outcome, Outcome::HandedOff);
}

#[test]
fn forged_actor_is_refused() {
    set_test_workspace();
    let store = Store::open_in_memory().expect("open store");
    let transport = FakeTransport { accepted: true };

    let mut req = request("body-1");
    req.actor = "forged-actor".to_string();
    let h = HandoffId::new_v4();
    let forged = Handoff {
        handoff_id: h,
        request: req,
    };

    let err = receive(&store, &forged, &transport).expect_err("must refuse");
    assert!(matches!(err, ReceiverError::ActorMismatch));

    // Nothing was persisted.
    assert_eq!(store.list_non_terminal().expect("list").len(), 0);
}

#[test]
fn drifted_body_is_refused() {
    set_test_workspace();
    let store = Store::open_in_memory().expect("open store");
    let transport = FakeTransport { accepted: true };

    // Persist body-1 in-flight, then re-deliver the same handoff id drifted to
    // body-2: the digest no longer matches → refusal with a mismatch.
    let req = request("body-1");
    let record = record_from(&req);
    let h = HandoffId::new_v4();
    let r1 = store.insert_pending(&record, &h).expect("insert in-flight");
    store
        .transition_to(&r1.receipt_id, Outcome::HandedOff, None)
        .expect("mark handed-off");

    let result = receive(&store, &handoff(h, "body-2"), &transport);
    match result {
        Err(ReceiverError::Refusal(mismatch)) => {
            // The drift is real: the stored digest id differs from the
            // presented one.
            assert_ne!(mismatch.stored_id, mismatch.presented_id);
        }
        other => panic!("expected Refusal, got {other:?}"),
    }
}

#[test]
fn handed_off_persists_and_sweeps_across_reopen() {
    set_test_workspace();
    let dir = TestDir::new();
    let path = dir.file();

    let (h, receipt_id);
    {
        let store = Store::open(&path).expect("open store");
        let req = request("body-1");
        let record = record_from(&req);
        h = HandoffId::new_v4();
        let r = store.insert_pending(&record, &h).expect("insert");
        store
            .transition_to(&r.receipt_id, Outcome::HandedOff, None)
            .expect("mark handed-off");
        receipt_id = r.receipt_id;
    } // drop closes the connection

    let store = Store::open(&path).expect("reopen store");
    let swept = startup_sweep(&store).expect("sweep");
    assert_eq!(swept.len(), 1);
    assert_eq!(swept[0].receipt_id, receipt_id);
    assert_eq!(swept[0].handoff_id, h);
    assert_eq!(swept[0].outcome, Outcome::HandedOff);
}

#[test]
fn two_racing_attempts_converge_on_one_active() {
    set_test_workspace();
    let dir = TestDir::new();
    let path = dir.file();

    // Prime WAL mode on the file before racing (avoids concurrent PRAGMA
    // journal_mode writes).
    drop(Store::open(&path).expect("prime store"));

    let req = request("body-1");
    let barrier = Arc::new(Barrier::new(2));

    let path_a = path.clone();
    let req_a = req.clone();
    let barrier_a = Arc::clone(&barrier);
    let a = std::thread::spawn(move || {
        let store = Store::open(&path_a).expect("open store a");
        let transport = FakeTransport { accepted: true };
        let handoff = Handoff {
            handoff_id: HandoffId::new_v4(),
            request: req_a,
        };
        barrier_a.wait();
        receive(&store, &handoff, &transport)
    });

    let path_b = path.clone();
    let req_b = req.clone();
    let barrier_b = Arc::clone(&barrier);
    let b = std::thread::spawn(move || {
        let store = Store::open(&path_b).expect("open store b");
        let transport = FakeTransport { accepted: true };
        let handoff = Handoff {
            handoff_id: HandoffId::new_v4(),
            request: req_b,
        };
        barrier_b.wait();
        receive(&store, &handoff, &transport)
    });

    let ra = a.join().expect("thread a did not panic");
    let rb = b.join().expect("thread b did not panic");

    // At least one attempt succeeds end to end.
    assert!(
        ra.is_ok() || rb.is_ok(),
        "at least one racing attempt succeeds: {ra:?} / {rb:?}"
    );

    // At most one active (non-terminal) receipt survives.
    let store = Store::open(&path).expect("reopen store");
    let active = store.list_non_terminal().expect("list non-terminal");
    assert!(active.len() <= 1, "at most one active attempt: {active:?}");
}
