//! Runnable smoke: the receiver lifecycle against a fake transport and a real
//! tempdir SQLite file.
//!
//! Prints the receipt id + outcome, shows the idempotent re-delivery no-op,
//! then a drifted re-delivery refusal.

use std::sync::atomic::{AtomicUsize, Ordering};

use herdr_board::{
    prove_actor, receive, CanonicalRequest, ExternalResponse, FactoryKind, Handoff, HandoffId,
    HandoffResult, HandoffTransport, Identity, ReceiverError, Store,
};

/// A transport that always accepts and counts calls.
struct AcceptingTransport {
    calls: AtomicUsize,
}

impl HandoffTransport for AcceptingTransport {
    fn handoff(&self, _request: &CanonicalRequest) -> HandoffResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        HandoffResult::Accepted(ExternalResponse::new("ok: scheduled by coordinator"))
    }
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "herdr-board-receiver-demo-{}-{}",
        std::process::id(),
        HandoffId::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn main() {
    let dir = temp_dir();
    let path = dir.join("herdr-board.sqlite3");
    let store = Store::open(&path).expect("open store");
    let transport = AcceptingTransport {
        calls: AtomicUsize::new(0),
    };

    let actor = prove_actor();
    let request = CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: Identity::new("ThalixInc", "herdr-board", 42),
        revision: "2026-09-12T00:00:00Z".into(),
        factory: "coordinator".into(),
        actor: actor.value,
        body: "build the board\nwith care".into(),
    };

    let handoff = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request.clone(),
    };

    // 1. Receive → accepted.
    let receipt = receive(&store, &handoff, &transport).expect("receive");
    println!("receipt id: {}", receipt.receipt_id);
    println!("outcome:    {}", receipt.outcome.as_str());

    // 2. Re-delivery (same handoff) → idempotent no-op.
    let again = receive(&store, &handoff, &transport).expect("re-delivery");
    println!(
        "re-delivery: same receipt ({})",
        again.receipt_id == receipt.receipt_id
    );
    println!(
        "transport calls after re-delivery: {}",
        transport.calls.load(Ordering::SeqCst)
    );

    // 3. Drift → refusal.
    let mut drifted = request;
    drifted.body = "build the board\nwith care (edited after persist)".into();
    let drifted_handoff = Handoff {
        handoff_id: receipt.handoff_id,
        request: drifted,
    };
    match receive(&store, &drifted_handoff, &transport) {
        Err(ReceiverError::Refusal(mismatch)) => {
            println!();
            println!("drifted re-delivery:");
            println!("{}", mismatch);
            println!("REFUSED");
        }
        other => panic!("expected drift refusal, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}
