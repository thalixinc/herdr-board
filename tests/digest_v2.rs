//! G5 schema-bump tests: v1↔v2 framing, the pinned golden vectors for both
//! layouts, version-on-record verification, and cross-version mismatch.

use herdr_board::digest::{
    compute, compute_with_version, verify, CanonicalRequest, Field, Identity, StoredFields,
    SCHEMA_VERSION,
};
use herdr_board::FactoryKind;

/// The v1 golden digest for the fixture below (five fields, no `factory_kind`).
const V1_GOLDEN: &str = "b6644706c905824decdd360e5dfdb6caf4e81090909c494583a0215cefa1a5e9";
/// The v2 golden digest (six fields, `factory_kind` first).
const V2_GOLDEN: &str = "9570b03cf8d9a02b93c2bc363283b7e6ed4479b955c86f63808b32eca470c8d6";

fn request(kind: FactoryKind) -> CanonicalRequest {
    CanonicalRequest {
        factory_kind: kind,
        identity: Identity::new("ThalixInc", "herdr-board", 42),
        revision: "r1".into(),
        factory: "coordinator".into(),
        actor: "founder".into(),
        body: "build the board\nwith care".into(),
    }
}

fn stored_fields_with_version(version: u64) -> StoredFields {
    StoredFields {
        schema_version: version,
        identity: Identity::new("ThalixInc", "herdr-board", 42),
        revision: "r1".into(),
        factory: "coordinator".into(),
        actor: "founder".into(),
        body: "build the board\nwith care".into(),
        factory_kind: FactoryKind::FactoryRequest,
    }
}

#[test]
fn v1_and_v2_golden_vectors_differ() {
    let req = request(FactoryKind::FactoryRequest);
    assert_eq!(compute_with_version(&req, 1).to_hex(), V1_GOLDEN);
    assert_eq!(compute_with_version(&req, 2).to_hex(), V2_GOLDEN);
    assert_ne!(
        compute_with_version(&req, 1),
        compute_with_version(&req, 2),
        "the schema bump must change every digest"
    );
}

#[test]
fn compute_uses_current_schema() {
    assert_eq!(SCHEMA_VERSION, 2);
    let req = request(FactoryKind::FactoryRequest);
    assert_eq!(compute(&req), compute_with_version(&req, SCHEMA_VERSION));
    assert_eq!(compute(&req).to_hex(), V2_GOLDEN);
}

#[test]
fn factory_kind_is_hashed_in_v2() {
    let factory = request(FactoryKind::FactoryRequest);
    let ordinary = request(FactoryKind::Ordinary);
    assert_ne!(compute(&factory), compute(&ordinary));
}

#[test]
fn v1_framing_ignores_factory_kind() {
    let factory = request(FactoryKind::FactoryRequest);
    let ordinary = request(FactoryKind::Ordinary);
    assert_eq!(
        compute_with_version(&factory, 1),
        compute_with_version(&ordinary, 1),
        "v1 predates the factory kind, so it is not hashed"
    );
}

#[test]
fn factory_kind_drift_yields_mismatch() {
    let req = request(FactoryKind::FactoryRequest);
    let stored = compute(&req);
    let stored_fields = StoredFields::from(&req);

    let mut drifted = req.clone();
    drifted.factory_kind = FactoryKind::Ordinary;

    let mismatch = verify(&stored, &stored_fields, &drifted).expect_err("kind drift must fail");
    assert_eq!(mismatch.field_diffs.len(), 1, "exactly the kind drifted");
    assert_eq!(mismatch.field_diffs[0].field, Field::FactoryKind);
    assert_ne!(mismatch.stored_id, mismatch.presented_id);
}

#[test]
fn version_on_record_verifies_v1() {
    let req = request(FactoryKind::FactoryRequest);
    let stored = compute_with_version(&req, 1);
    let stored_fields = stored_fields_with_version(1);
    verify(&stored, &stored_fields, &req).expect("a v1 record verifies under the v1 layout");
}

#[test]
fn version_on_record_verifies_v2() {
    let req = request(FactoryKind::FactoryRequest);
    let stored = compute(&req);
    let stored_fields = StoredFields::from(&req);
    verify(&stored, &stored_fields, &req).expect("a v2 record verifies under the v2 layout");
}

#[test]
fn cross_version_mismatch() {
    let req = request(FactoryKind::FactoryRequest);

    // A v1 digest presented against a v2 record (and vice-versa) is a refusal.
    let v1_digest = compute_with_version(&req, 1);
    let v2_fields = StoredFields::from(&req); // schema_version = 2
    let mismatch =
        verify(&v1_digest, &v2_fields, &req).expect_err("v1 digest must not verify under v2");
    assert!(
        mismatch.field_diffs.is_empty(),
        "no field drifted; the layout changed"
    );

    let v2_digest = compute_with_version(&req, 2);
    let v1_fields = stored_fields_with_version(1);
    assert!(
        verify(&v2_digest, &v1_fields, &req).is_err(),
        "v2 digest must not verify under v1"
    );
}
