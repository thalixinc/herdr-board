//! Integration: the full receiver lifecycle against a fake transport.
//!
//! Pins the receiver's core guarantees: clean dispatch, idempotent
//! re-delivery, in-flight dedup, actor provenance, drift refusal, crash
//! recovery (write-ahead), and the single-active-attempt invariant.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use herdr_board::{
    compute, prove_actor, receive, reconfirm, replay, startup_sweep, status_query,
    CanonicalRequest, ExternalResponse, FactoryKind, Field, Handoff, HandoffId, HandoffResult,
    HandoffTransport, Identity, Outcome, ReceiverError, RequestRecord, Store, SCHEMA_VERSION,
    STALENESS_THRESHOLD,
};

/// A fake transport that returns a fixed decision and counts calls.
struct FakeTransport {
    calls: Arc<AtomicUsize>,
    decision: Decision,
}

enum Decision {
    Accept,
    Refuse,
    Fail,
}

impl HandoffTransport for FakeTransport {
    fn handoff(&self, _request: &CanonicalRequest) -> HandoffResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.decision {
            Decision::Accept => HandoffResult::Accepted(ExternalResponse::new("ok: scheduled")),
            Decision::Refuse => HandoffResult::Refused(ExternalResponse::new("no: quota")),
            Decision::Fail => HandoffResult::Failed("coordinator unreachable".to_owned()),
        }
    }
}

fn request(body: &str) -> CanonicalRequest {
    let actor = prove_actor();
    CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: Identity::new("ThalixInc", "herdr-board", 42),
        revision: "2026-09-12T00:00:00Z".into(),
        factory: "coordinator".into(),
        actor: actor.value,
        body: body.to_owned(),
    }
}

fn handoff(request: CanonicalRequest) -> Handoff {
    Handoff {
        handoff_id: HandoffId::new_v4(),
        request,
    }
}

fn accepting() -> FakeTransport {
    FakeTransport {
        calls: Arc::new(AtomicUsize::new(0)),
        decision: Decision::Accept,
    }
}

fn failing() -> FakeTransport {
    FakeTransport {
        calls: Arc::new(AtomicUsize::new(0)),
        decision: Decision::Fail,
    }
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "herdr-board-receiver-test-{}-{}",
        std::process::id(),
        HandoffId::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn clean_path_accepts() {
    let store = Store::open_in_memory().unwrap();
    let transport = accepting();
    let receipt = receive(&store, &handoff(request("build the board")), &transport).unwrap();
    assert_eq!(receipt.outcome, Outcome::Accepted);
    assert_eq!(receipt.external_response.as_deref(), Some("ok: scheduled"));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn coordinator_refusal_is_a_terminal_receipt() {
    let store = Store::open_in_memory().unwrap();
    let transport = FakeTransport {
        calls: Arc::new(AtomicUsize::new(0)),
        decision: Decision::Refuse,
    };
    let receipt = receive(&store, &handoff(request("build the board")), &transport).unwrap();
    assert_eq!(receipt.outcome, Outcome::Refused);
    assert_eq!(receipt.external_response.as_deref(), Some("no: quota"));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn idempotent_redelivery_returns_same_receipt() {
    let store = Store::open_in_memory().unwrap();
    let transport = accepting();
    let handoff = handoff(request("build the board"));

    let first = receive(&store, &handoff, &transport).unwrap();
    let second = receive(&store, &handoff, &transport).unwrap();

    assert_eq!(first.receipt_id, second.receipt_id);
    assert_eq!(second.outcome, Outcome::Accepted);
    // The transport ran exactly once — the re-delivery did not re-dispatch.
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn inflight_dedup_returns_active_receipt() {
    let store = Store::open_in_memory().unwrap();
    let transport = failing();
    let req = request("build the board");

    // First attempt: transport fails, so the receipt stays handed-off (active).
    let err = receive(&store, &handoff(req.clone()), &transport).unwrap_err();
    assert!(matches!(err, ReceiverError::Transport(_)));

    let active = store.get_active_by_digest(&compute(&req)).unwrap().unwrap();
    assert_eq!(active.outcome, Outcome::HandedOff);

    // Same input, new handoff id → in-flight dedup returns the active receipt.
    let err = receive(&store, &handoff(req), &transport).unwrap_err();
    match err {
        ReceiverError::ActiveAttemptExists { receipt } => {
            assert_eq!(receipt.receipt_id, active.receipt_id);
        }
        other => panic!("expected ActiveAttemptExists, got {other:?}"),
    }
}

#[test]
fn forged_actor_is_refused() {
    let store = Store::open_in_memory().unwrap();
    let transport = accepting();
    let mut req = request("build the board");
    let forged = format!("{}!forged", prove_actor().value);
    req.actor = forged.clone();

    let err = receive(&store, &handoff(req), &transport).unwrap_err();
    match err {
        ReceiverError::ActorMismatch { claimed, .. } => assert_eq!(claimed, forged),
        other => panic!("expected ActorMismatch, got {other:?}"),
    }
    // Nothing was dispatched.
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn drifted_body_is_refused_with_mismatch() {
    let store = Store::open_in_memory().unwrap();
    let transport = accepting();
    let original = request("build the board");

    // Clean dispatch → accepted.
    let first = receive(&store, &handoff(original.clone()), &transport).unwrap();
    assert_eq!(first.outcome, Outcome::Accepted);

    // Re-deliver the SAME handoff id with a drifted body.
    let mut drifted = original;
    drifted.body = "build the board (edited)".to_owned();
    let re = Handoff {
        handoff_id: first.handoff_id,
        request: drifted,
    };
    let err = receive(&store, &re, &transport).unwrap_err();
    match err {
        ReceiverError::Refusal(mismatch) => {
            assert_eq!(mismatch.field_diffs.len(), 1);
            let diff = &mismatch.field_diffs[0];
            assert_eq!(diff.field, Field::Body);
            assert_eq!(diff.old, "build the board");
            assert_eq!(diff.new, "build the board (edited)");
            assert_ne!(mismatch.stored_id, mismatch.presented_id);
        }
        other => panic!("expected Refusal(Mismatch), got {other:?}"),
    }

    // The original receipt is untouched (never rewritten after a failed verify).
    let stored = store.get_by_handoff_id(&first.handoff_id).unwrap().unwrap();
    assert_eq!(stored.receipt_id, first.receipt_id);
    assert_eq!(stored.outcome, Outcome::Accepted);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn handed_off_persists_across_reopen_and_sweep_surfaces_it() {
    let dir = temp_dir();
    let path = dir.join("herdr-board.sqlite3");
    let transport = failing();
    let req = request("build the board");
    let handoff_id;

    {
        let store = Store::open(&path).unwrap();
        let h = handoff(req.clone());
        handoff_id = h.handoff_id;
        // Transport fails: receipt stays handed-off. Dropping `store` here
        // simulates a crash between the `handed-off` commit and finalize.
        let err = receive(&store, &h, &transport).unwrap_err();
        assert!(matches!(err, ReceiverError::Transport(_)));
    }

    // Reopen: the handed-off receipt persisted.
    let store = Store::open(&path).unwrap();
    let receipt = store.get_by_handoff_id(&handoff_id).unwrap().unwrap();
    assert_eq!(receipt.outcome, Outcome::HandedOff);

    // Sweep with "now" past the staleness threshold surfaces it.
    let now = receipt.created_at + STALENESS_THRESHOLD.as_secs() as i64 + 1;
    let stale = startup_sweep(&store, now).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].receipt_id, receipt.receipt_id);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_racing_receives_yield_one_active_receipt() {
    let store = Store::open_in_memory().unwrap();
    let transport = failing();
    let req = request("build the board");

    // Simulate a racing writer that already acquired the active-attempt slot.
    let record = RequestRecord {
        schema_version: SCHEMA_VERSION,
        factory_kind: req.factory_kind,
        digest: compute(&req),
        identity: req.identity.canonical(),
        revision: req.revision.clone(),
        factory: req.factory.clone(),
        actor: req.actor.clone(),
        actor_source: "os-user".to_owned(),
        body: req.body.clone(),
    };
    let existing = store.insert_pending(&record, &HandoffId::new_v4()).unwrap();

    // A concurrent receive of the same input must not create a second attempt.
    let err = receive(&store, &handoff(req), &transport).unwrap_err();
    match err {
        ReceiverError::ActiveAttemptExists { receipt } => {
            assert_eq!(receipt.receipt_id, existing.receipt_id);
        }
        other => panic!("expected ActiveAttemptExists, got {other:?}"),
    }

    // Exactly one non-terminal receipt exists.
    let active = store.list_non_terminal().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].receipt_id, existing.receipt_id);
}

#[test]
fn reconfirm_redispatches_handed_off_receipt() {
    let store = Store::open_in_memory().unwrap();
    let fail = failing();
    let accept = accepting();
    let req = request("build the board");
    let h = handoff(req.clone());

    let err = receive(&store, &h, &fail).unwrap_err();
    assert!(matches!(err, ReceiverError::Transport(_)));
    let handed_off = store.get_by_handoff_id(&h.handoff_id).unwrap().unwrap();
    assert_eq!(handed_off.outcome, Outcome::HandedOff);

    // Re-confirm with a healthy transport: same receipt, now accepted.
    let reaccepted = reconfirm(&store, &handed_off, &accept).unwrap();
    assert_eq!(reaccepted.receipt_id, handed_off.receipt_id);
    assert_eq!(reaccepted.outcome, Outcome::Accepted);
    assert_eq!(accept.calls.load(Ordering::SeqCst), 1);

    // Re-confirming again is refused: terminal outcomes are final.
    assert!(matches!(
        reconfirm(&store, &reaccepted, &accept),
        Err(ReceiverError::Store(_))
    ));
}

#[test]
fn status_query_records_answer_and_replay_is_noop() {
    let store = Store::open_in_memory().unwrap();
    let fail = failing();
    let req = request("build the board");
    let h = handoff(req.clone());

    let _ = receive(&store, &h, &fail).unwrap_err();
    let handed_off = store.get_by_handoff_id(&h.handoff_id).unwrap().unwrap();

    // Record an out-of-band external answer (accepted).
    let answered = status_query(
        &store,
        &handed_off,
        true,
        &ExternalResponse::new("ok: acked"),
    )
    .unwrap();
    assert_eq!(answered.outcome, Outcome::Accepted);
    assert_eq!(answered.external_response.as_deref(), Some("ok: acked"));

    // Replay: idempotent no-op — returns the receipt unchanged.
    let replayed = replay(&store, &answered).unwrap();
    assert_eq!(replayed, answered);
}
