//! Runnable smoke of the receiver: a clean handoff, then a drifted re-delivery
//! that is refused. Runs against a real tempdir SQLite file.

use std::path::PathBuf;

use herdr_board::digest::CanonicalRequest;
use herdr_board::outbox::{HandoffId, Store};
use herdr_board::receiver::{
    receive, ExternalResponse, Handoff, HandoffResult, HandoffTransport, ReceiverError, TrustRoot,
};

struct Accepting;

impl HandoffTransport for Accepting {
    fn handoff(&self, _handoff: &Handoff) -> HandoffResult {
        HandoffResult {
            accepted: true,
            response: ExternalResponse("ack-ok".to_string()),
        }
    }
}

fn request(body: &str) -> CanonicalRequest {
    let actor = TrustRoot::from_env().value;
    CanonicalRequest {
        identity: herdr_board::Identity::new("thalixinc", "herdr-board", 42),
        revision: "r1".to_string(),
        factory: "coordinator".to_string(),
        actor,
        body: body.to_string(),
    }
}

fn main() {
    let dir =
        std::env::temp_dir().join(format!("herdr-board-receiver-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path: PathBuf = dir.join("demo.sqlite3");

    let store = Store::open(&path).expect("open store");
    let transport = Accepting;

    let handoff_id = HandoffId::new_v4();
    let clean = Handoff {
        handoff_id,
        request: request("build the board"),
    };

    // Clean path.
    match receive(&store, &clean, &transport) {
        Ok(receipt) => println!(
            "receipt {} outcome {}",
            receipt.receipt_id,
            receipt.outcome.as_str()
        ),
        Err(e) => {
            eprintln!("clean handoff failed: {e}");
            std::process::exit(1);
        }
    }

    // Drift: same handoff id, edited body → the digest no longer matches.
    let drifted = Handoff {
        handoff_id,
        request: request("build the board (edited)"),
    };
    match receive(&store, &drifted, &transport) {
        Ok(_) => {
            eprintln!("expected refusal, got a receipt");
            std::process::exit(1);
        }
        Err(ReceiverError::Refusal(mismatch)) => {
            println!(
                "REFUSED: stored {} != presented {}",
                mismatch.stored_id, mismatch.presented_id
            );
        }
        Err(e) => {
            eprintln!("unexpected error: {e}");
            std::process::exit(1);
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
