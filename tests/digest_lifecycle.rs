//! Integration: persist → dispatch → acceptance, and drift → refusal.
//!
//! Pins the ticket's core guarantee: the input persisted for "Process with
//! factory" is byte-for-byte the input dispatched and accepted, and any drift
//! surfaces as a field-level [`Mismatch`] — with the stored digest never
//! rewritten after a failed verify.

use herdr_board::digest::{
    compute, verify, CanonicalRequest, Field, Identity, Refusal, StoredFields,
};
use herdr_board::FactoryKind;

fn request() -> CanonicalRequest {
    CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: Identity::new("ThalixInc", "herdr-board", 42),
        revision: "2026-09-12T00:00:00Z".into(),
        factory: "coordinator".into(),
        actor: "founder@thalix".into(),
        body: "build the board\nwith care".into(),
    }
}

#[test]
fn clean_lifecycle_persist_dispatch_acceptance() {
    let req = request();

    // Persist: compute once, snapshot the fields alongside the digest.
    let stored = compute(&req);
    let stored_fields = StoredFields::from(&req);

    // Dispatch and acceptance both verify clean.
    verify(&stored, &stored_fields, &req).expect("dispatch must verify clean");
    verify(&stored, &stored_fields, &req).expect("acceptance must verify clean");
}

#[test]
fn body_drift_at_dispatch_yields_field_mismatch() {
    let req = request();
    let stored = compute(&req);
    let stored_fields = StoredFields::from(&req);

    let mut drifted = req.clone();
    drifted.body = "build the board\nwith care (edited)".into();

    let mismatch =
        verify(&stored, &stored_fields, &drifted).expect_err("a drifted body must not verify");

    assert_eq!(mismatch.field_diffs.len(), 1, "exactly the body drifted");
    let diff = &mismatch.field_diffs[0];
    assert_eq!(diff.field, Field::Body);
    assert_eq!(diff.old, "build the board\nwith care");
    assert_eq!(diff.new, "build the board\nwith care (edited)");

    // The two digest ids differ, and each points at the right input.
    assert_ne!(mismatch.stored_id, mismatch.presented_id);
    assert_eq!(mismatch.stored_id, stored.digest_id());
    assert_eq!(mismatch.presented_id, compute(&drifted).digest_id());
}

#[test]
fn revision_drift_at_acceptance_yields_field_mismatch() {
    let req = request();
    let stored = compute(&req);
    let stored_fields = StoredFields::from(&req);

    // Dispatch verifies clean; then the issue's revision bumps before acceptance.
    verify(&stored, &stored_fields, &req).expect("dispatch must verify clean");

    let mut drifted = req.clone();
    drifted.revision = "2026-09-12T01:00:00Z".into();

    let mismatch =
        verify(&stored, &stored_fields, &drifted).expect_err("a drifted revision must not verify");

    assert_eq!(
        mismatch.field_diffs.len(),
        1,
        "exactly the revision drifted"
    );
    assert_eq!(mismatch.field_diffs[0].field, Field::Revision);
    assert_eq!(mismatch.field_diffs[0].old, "2026-09-12T00:00:00Z");
    assert_eq!(mismatch.field_diffs[0].new, "2026-09-12T01:00:00Z");
}

#[test]
fn stored_digest_is_never_rewritten_after_failed_verify() {
    let req = request();
    let stored = compute(&req);
    let stored_fields = StoredFields::from(&req);
    let persisted = stored; // `Digest` is `Copy`; `verify` borrows, never mutates.

    let mut drifted = req.clone();
    drifted.body = "drifted".into();
    let mismatch = verify(&stored, &stored_fields, &drifted).expect_err("drift must fail");

    // The stored digest is unchanged, still equals the original input, and the
    // original input still verifies — no recompute-and-overwrite happened.
    assert_eq!(
        stored, persisted,
        "a failed verify must never rewrite the stored digest"
    );
    assert_eq!(stored, compute(&req));
    verify(&stored, &stored_fields, &req).expect("the original request still verifies");

    // The mismatch records the drift, not a repaired digest.
    assert_ne!(mismatch.stored_id, mismatch.presented_id);
    assert_ne!(mismatch.presented_id, stored.digest_id());
}

#[test]
fn multiple_fields_drift_together() {
    let req = request();
    let stored = compute(&req);
    let stored_fields = StoredFields::from(&req);

    let mut drifted = req.clone();
    drifted.revision = "bumped".into();
    drifted.factory = "planner".into();

    let mismatch = verify(&stored, &stored_fields, &drifted).expect_err("drift must fail");
    let fields: Vec<Field> = mismatch.field_diffs.iter().map(|d| d.field).collect();
    assert_eq!(fields, vec![Field::Revision, Field::Factory]);
}

#[test]
fn refusal_outcome_wraps_the_mismatch() {
    let req = request();
    let stored = compute(&req);
    let stored_fields = StoredFields::from(&req);

    let mut drifted = req;
    drifted.actor = "someone-else".into();

    let mismatch = verify(&stored, &stored_fields, &drifted).expect_err("drift must fail");
    assert_eq!(mismatch.field_diffs[0].field, Field::Actor);

    let refusal = Refusal::from(mismatch);
    assert_eq!(refusal.mismatch.field_diffs[0].field, Field::Actor);
    assert_eq!(refusal.to_string(), "REFUSED: factory will not run");
}
