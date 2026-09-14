//! Integration: the real transport translation + reconciliation entrypoints.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use herdr_board::{
    cancel_receipt, compute, herdr_axi_send_with_bin, prove_actor, receive, reconfirm,
    startup_sweep, status_query, AxiTarget, CanonicalRequest, CfQueueContract, CfSubmission,
    FactoryKind, Handoff, HandoffId, HandoffResult, HandoffTransport, Identity, Outcome,
    RealHandoffTransport, ReceiverError, RequestRecord, Store, SCHEMA_VERSION, STALENESS_THRESHOLD,
};

mod common;

/// A fake transport that returns a fixed decision and counts calls.
struct FakeTransport {
    calls: Arc<AtomicUsize>,
    decision: Decision,
}

#[derive(Clone, Copy)]
enum Decision {
    Accept,
    Fail,
}

impl HandoffTransport for FakeTransport {
    fn handoff(&self, _request: &CanonicalRequest) -> HandoffResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.decision {
            Decision::Accept => {
                HandoffResult::Accepted(herdr_board::ExternalResponse::new("ok: scheduled"))
            }
            Decision::Fail => HandoffResult::Failed("unreachable".to_owned()),
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

#[test]
fn sweep_surfaces_only_stale_handed_off() {
    let store = Store::open_in_memory().unwrap();
    let req = request("build the board");
    let h = handoff(req.clone());
    let err = receive(&store, &h, &failing()).unwrap_err();
    assert!(matches!(err, ReceiverError::Transport(_)));

    let receipt = store.get_by_handoff_id(&h.handoff_id).unwrap().unwrap();
    assert_eq!(receipt.outcome, Outcome::HandedOff);

    // Not stale yet.
    assert!(startup_sweep(&store, receipt.created_at).is_empty());
    // Stale: past the threshold.
    let now = receipt.created_at + STALENESS_THRESHOLD.as_secs() as i64 + 1;
    let stale = startup_sweep(&store, now);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].receipt_id, receipt.receipt_id);
}

#[test]
fn sweep_ignores_pending_receipts() {
    let store = Store::open_in_memory().unwrap();
    let req = request("build the board");

    // A pending (not handed-off) receipt must never surface, even when old.
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
    let pending = store.insert_pending(&record, &HandoffId::new_v4()).unwrap();
    assert_eq!(pending.outcome, Outcome::Pending);

    let now = pending.created_at + STALENESS_THRESHOLD.as_secs() as i64 + 1;
    assert!(startup_sweep(&store, now).is_empty());
}

#[test]
fn reconfirm_redispatches_idempotently() {
    let store = Store::open_in_memory().unwrap();
    let req = request("build the board");
    let h = handoff(req.clone());

    let err = receive(&store, &h, &failing()).unwrap_err();
    assert!(matches!(err, ReceiverError::Transport(_)));
    let handed_off = store.get_by_handoff_id(&h.handoff_id).unwrap().unwrap();

    let accept = accepting();
    let reaccepted = reconfirm(&store, &handed_off.receipt_id, &accept).unwrap();
    assert_eq!(reaccepted.receipt_id, handed_off.receipt_id);
    assert_eq!(reaccepted.outcome, Outcome::Accepted);
    assert_eq!(accept.calls.load(Ordering::SeqCst), 1);

    // Terminal: re-confirming again is refused (never double-accepted).
    assert!(matches!(
        reconfirm(&store, &reaccepted.receipt_id, &accept),
        Err(ReceiverError::Store(_))
    ));
}

#[test]
fn status_query_records_and_transitions() {
    let store = Store::open_in_memory().unwrap();
    let req = request("build the board");
    let h = handoff(req.clone());
    let _ = receive(&store, &h, &failing()).unwrap_err();
    let handed_off = store.get_by_handoff_id(&h.handoff_id).unwrap().unwrap();

    let answered = status_query(&store, &handed_off.receipt_id, true, "external ack").unwrap();
    assert_eq!(answered.outcome, Outcome::Accepted);
    assert_eq!(answered.external_response.as_deref(), Some("external ack"));
}

#[test]
fn cancel_marks_receipt_terminal() {
    let store = Store::open_in_memory().unwrap();
    let req = request("build the board");
    let h = handoff(req.clone());
    let _ = receive(&store, &h, &failing()).unwrap_err();
    let handed_off = store.get_by_handoff_id(&h.handoff_id).unwrap().unwrap();

    let cancelled = cancel_receipt(&store, &handed_off.receipt_id).unwrap();
    assert_eq!(cancelled.outcome, Outcome::Cancelled);
    // Terminal: no longer surfaced.
    assert!(startup_sweep(&store, cancelled.created_at + 1000).is_empty());
}

#[test]
fn contract_translation_maps_fields_verbatim() {
    let req = request("payload\nwith newline");
    let contract = CfQueueContract::from_request(&req);
    assert_eq!(contract.identity, "thalixinc/herdr-board#42"); // canonicalized
    assert_eq!(contract.revision, req.revision);
    assert_eq!(contract.factory, req.factory);
    assert_eq!(contract.actor, req.actor);
    assert_eq!(contract.body, "payload\nwith newline"); // verbatim, not re-encoded
}

#[test]
fn real_transport_maps_three_way() {
    let req = request("body");

    let accepting = RealHandoffTransport::new(|_c, _id| CfSubmission::Accepted {
        response: "queued".to_owned(),
    });
    match accepting.handoff(&req) {
        HandoffResult::Accepted(resp) => assert_eq!(resp.as_str(), "queued"),
        other => panic!("expected Accepted, got {other:?}"),
    }

    let refusing = RealHandoffTransport::new(|_c, _id| CfSubmission::Refused {
        response: "denied".to_owned(),
    });
    match refusing.handoff(&req) {
        HandoffResult::Refused(resp) => assert_eq!(resp.as_str(), "denied"),
        other => panic!("expected Refused, got {other:?}"),
    }

    let failing = RealHandoffTransport::new(|_c, _id| CfSubmission::Failed {
        reason: "timeout".to_owned(),
    });
    match failing.handoff(&req) {
        HandoffResult::Failed(reason) => assert_eq!(reason, "timeout"),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn herdr_axi_submitted_maps_accepted() {
    let dir = common::scratch_dir("transport-submitted");
    let capture = dir.join("capture.jsonl");
    let bin = common::fake_herdr_axi(&dir, "submitted", &capture);
    let target = AxiTarget::new("herdr-board", "coordinator", None);
    let contract = CfQueueContract::from_request(&request("build the board"));

    match herdr_axi_send_with_bin(bin.to_str().unwrap(), &target, &contract, "corr-1") {
        CfSubmission::Accepted { response } => {
            // The `submitted` result is carried verbatim (correlation id +
            // coordinator state) so `status_query` can resolve it later.
            let result: serde_json::Value =
                serde_json::from_str(&response).expect("submitted result is JSON");
            assert_eq!(result["state"], "idle");
        }
        other => panic!("expected Accepted, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn herdr_axi_not_submitted_maps_failed() {
    let dir = common::scratch_dir("transport-not-submitted");
    let capture = dir.join("capture.jsonl");
    let bin = common::fake_herdr_axi(&dir, "not-submitted", &capture);
    let target = AxiTarget::new("herdr-board", "coordinator", None);
    let contract = CfQueueContract::from_request(&request("build the board"));

    match herdr_axi_send_with_bin(bin.to_str().unwrap(), &target, &contract, "corr-1") {
        CfSubmission::Failed { reason } => {
            assert!(
                reason.starts_with("not-submitted:"),
                "reason was {reason:?}"
            )
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn herdr_axi_unknown_maps_failed() {
    let dir = common::scratch_dir("transport-unknown");
    let capture = dir.join("capture.jsonl");
    let bin = common::fake_herdr_axi(&dir, "unknown", &capture);
    let target = AxiTarget::new("herdr-board", "coordinator", None);
    let contract = CfQueueContract::from_request(&request("build the board"));

    match herdr_axi_send_with_bin(bin.to_str().unwrap(), &target, &contract, "corr-1") {
        CfSubmission::Failed { reason } => {
            assert!(reason.starts_with("unknown:"), "reason was {reason:?}")
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn herdr_axi_body_carries_all_five_fields_verbatim() {
    let dir = common::scratch_dir("transport-body");
    let capture = dir.join("capture.jsonl");
    let bin = common::fake_herdr_axi(&dir, "submitted", &capture);
    let target = AxiTarget::new("herdr-board", "coordinator", None);
    let req = request("payload\nwith newline");
    let contract = CfQueueContract::from_request(&req);

    let _ = herdr_axi_send_with_bin(bin.to_str().unwrap(), &target, &contract, "corr-1");

    let envelopes = common::captured_envelopes(&capture);
    assert_eq!(envelopes.len(), 1, "one dispatch → one envelope");

    let text = envelopes[0]["params"]["text"]
        .as_str()
        .expect("params.text is a string");
    let body: serde_json::Value =
        serde_json::from_str(text).expect("body is the serialized contract");

    // All five fields, verbatim (identity canonicalized by `from_request`).
    assert_eq!(body["identity"], contract.identity);
    assert_eq!(body["revision"], contract.revision);
    assert_eq!(body["factory"], contract.factory);
    assert_eq!(body["actor"], contract.actor);
    assert_eq!(body["body"], contract.body);

    // The per-attempt correlation id travels with the envelope.
    assert_eq!(
        envelopes[0]["params"]["message_id"].as_str(),
        Some("corr-1")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn herdr_axi_identical_content_gets_distinct_message_ids() {
    let dir = common::scratch_dir("transport-distinct-id");
    let capture = dir.join("capture.jsonl");
    let bin = common::fake_herdr_axi(&dir, "submitted", &capture);
    let target = AxiTarget::new("herdr-board", "coordinator", None);
    let bin = bin.to_str().unwrap().to_owned();
    // The transport generates a fresh correlation id per attempt.
    let transport = RealHandoffTransport::new(move |c, id| {
        herdr_axi_send_with_bin(bin.as_str(), &target, c, id)
    });

    let req = request("build the board");
    transport.handoff(&req);
    transport.handoff(&req);

    let envelopes = common::captured_envelopes(&capture);
    assert_eq!(envelopes.len(), 2, "one dispatch per attempt");
    let first_id = envelopes[0]["params"]["message_id"]
        .as_str()
        .expect("message_id is a string");
    let second_id = envelopes[1]["params"]["message_id"]
        .as_str()
        .expect("message_id is a string");
    assert_ne!(
        first_id, second_id,
        "identical content still yields a fresh correlation id per attempt"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
