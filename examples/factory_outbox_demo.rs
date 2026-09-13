//! Runnable smoke: process → receipt, then a stopped factory (Failed) handoff
//! → handed-off → sweep surfaces → reconfirm resolves.

use herdr_board::{
    prove_actor, receive, reconfirm, startup_sweep, CanonicalRequest, FactoryKind, Handoff,
    HandoffId, HandoffResult, HandoffTransport, Identity, Store, STALENESS_THRESHOLD,
};

/// A healthy transport that always accepts.
struct Accepting;

impl HandoffTransport for Accepting {
    fn handoff(&self, _request: &CanonicalRequest) -> HandoffResult {
        HandoffResult::Accepted(herdr_board::ExternalResponse::new("scheduled"))
    }
}

/// A stopped factory: the transport cannot reach cf-queue.
struct Stopped;

impl HandoffTransport for Stopped {
    fn handoff(&self, _request: &CanonicalRequest) -> HandoffResult {
        HandoffResult::Failed("cf-queue unreachable".to_owned())
    }
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "herdr-board-factory-demo-{}-{}",
        std::process::id(),
        HandoffId::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn main() {
    let dir = temp_dir();
    let store = Store::open(dir.join("herdr-board.sqlite3")).expect("open store");

    let actor = prove_actor();
    let request = |body: &str| CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: Identity::new("ThalixInc", "herdr-board", 42),
        revision: "2026-09-12T00:00:00Z".into(),
        factory: "coordinator".into(),
        actor: actor.value.clone(),
        body: body.to_owned(),
    };

    // 1. Process with factory → accepted receipt.
    let h1 = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request("build the board"),
    };
    let receipt = receive(&store, &h1, &Accepting).expect("receive");
    println!(
        "receipt {} outcome={}",
        receipt.receipt_id,
        receipt.outcome.as_str()
    );

    // 2. Stopped factory: transport Failed → receipt stays handed-off.
    let h2 = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request("build the board"),
    };
    let err = receive(&store, &h2, &Stopped).unwrap_err();
    println!("stopped factory: {err:?}");
    let handed_off = store.get_by_handoff_id(&h2.handoff_id).unwrap().unwrap();
    println!(
        "receipt {} outcome={}",
        handed_off.receipt_id,
        handed_off.outcome.as_str()
    );

    // 3. Sweep surfaces the stale handed-off receipt (never re-dispatches).
    let now = handed_off.created_at + STALENESS_THRESHOLD.as_secs() as i64 + 1;
    let stale = startup_sweep(&store, now);
    println!("sweep: {} stale handed-off receipt(s)", stale.len());
    for receipt in &stale {
        println!(
            "  {} ({}s old)",
            receipt.receipt_id,
            now - receipt.created_at
        );
    }

    // 4. Reconfirm re-dispatches on a healthy transport and resolves it.
    let resolved = reconfirm(&store, &handed_off.receipt_id, &Accepting).expect("reconfirm");
    println!(
        "reconfirmed: {} outcome={}",
        resolved.receipt_id,
        resolved.outcome.as_str()
    );

    let _ = std::fs::remove_dir_all(&dir);
}
