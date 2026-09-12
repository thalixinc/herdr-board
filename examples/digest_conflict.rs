//! Runnable smoke: persist → dispatch → acceptance, then a drifted re-dispatch.
//!
//! Prints the persisted 16-hex digest id, confirms the clean lifecycle, then
//! shows the field-level conflict and `REFUSED` for the drifted re-dispatch.

use herdr_board::digest::{compute, verify, CanonicalRequest, Identity, Refusal, StoredFields};

fn main() {
    let request = CanonicalRequest {
        identity: Identity::new("ThalixInc", "herdr-board", 42),
        revision: "2026-09-12T00:00:00Z".into(),
        factory: "coordinator".into(),
        actor: "founder@thalix".into(),
        body: "build the board\nwith care".into(),
    };

    // Persist: compute once, snapshot the fields alongside the digest.
    let stored = compute(&request);
    let stored_fields = StoredFields::from(&request);
    println!("persisted digest id: {}", stored.digest_id());

    // Dispatch and acceptance verify clean.
    verify(&stored, &stored_fields, &request).expect("dispatch");
    verify(&stored, &stored_fields, &request).expect("acceptance");
    println!("dispatch + acceptance: verified clean");

    // Drift the body after persist; a re-dispatch must be refused.
    let mut drifted = request.clone();
    drifted.body = "build the board\nwith care (edited after persist)".into();

    let mismatch =
        verify(&stored, &stored_fields, &drifted).expect_err("drifted re-dispatch must mismatch");
    let refusal = Refusal::from(mismatch);

    println!();
    println!("drifted re-dispatch:");
    println!("{}", refusal.mismatch);
    println!("{}", refusal);
}
