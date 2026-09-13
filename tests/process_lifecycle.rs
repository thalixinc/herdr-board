//! "Process with factory" lifecycle (VS3): the promote → resolve → prove →
//! build → receive composition, and the identity-scoped convergence guarantee.

use herdr_board::{
    compute, create_draft, process_with_factory, prove_actor, receive, CanonicalFields,
    CanonicalRequest, Card, CardError, Conflict, ExternalResponse, FactoryKind, Handoff, HandoffId,
    HandoffResult, HandoffTransport, Identity, Outcome, ProcessError, ReceiverError, Store,
};

fn set_workspace() {
    std::env::set_var("HERDR_WORKSPACE_ID", "test-workspace");
}

fn identity() -> Identity {
    Identity::new("thalixinc", "herdr-board", 42)
}

fn card(identity: &Identity, revision: &str) -> Card {
    Card {
        identity: identity.clone(),
        url: "https://github.com/thalixinc/herdr-board/issues/42".to_string(),
        fields: CanonicalFields {
            title: "issue title".to_string(),
            body: "issue body (synced from GitHub)".to_string(),
            state: "open".to_string(),
            state_reason: None,
            labels: vec!["bug".to_string()],
            assignee: None,
            milestone: None,
        },
        column: "to-do".to_string(),
        factory_kind: FactoryKind::Ordinary,
        revision: revision.to_string(),
        conflict: Conflict::None,
        synced_at: 1000,
    }
}

fn draft(store: &Store, body: &str) -> herdr_board::Draft {
    create_draft(store, FactoryKind::Ordinary, "card title", body).expect("create draft")
}

/// Build the exact six-field request `process_with_factory` composes, for the
/// digest assertion.
fn expected_request(
    identity: &Identity,
    revision: &str,
    target: &str,
    body: &str,
) -> CanonicalRequest {
    let actor = prove_actor();
    CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: identity.clone(),
        revision: revision.to_string(),
        factory: target.to_string(),
        actor: actor.value,
        body: body.to_string(),
    }
}

fn request(identity: &Identity, revision: &str, body: &str) -> CanonicalRequest {
    let actor = prove_actor();
    CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: identity.clone(),
        revision: revision.to_string(),
        factory: "coordinator".to_string(),
        actor: actor.value,
        body: body.to_string(),
    }
}

#[derive(Clone, Copy)]
enum Decision {
    Accept,
    Fail,
}

struct FakeTransport {
    decision: Decision,
}

impl HandoffTransport for FakeTransport {
    fn handoff(&self, _request: &CanonicalRequest) -> HandoffResult {
        match self.decision {
            Decision::Accept => HandoffResult::Accepted(ExternalResponse::new("ok: scheduled")),
            Decision::Fail => HandoffResult::Failed("coordinator unreachable".to_string()),
        }
    }
}

fn accept() -> FakeTransport {
    FakeTransport {
        decision: Decision::Accept,
    }
}

fn fail() -> FakeTransport {
    FakeTransport {
        decision: Decision::Fail,
    }
}

#[test]
fn clean_process_builds_receipt_over_real_fields() {
    set_workspace();
    let store = Store::open_in_memory().expect("open store");
    let ident = identity();
    store
        .insert_card(&card(&ident, "rev-1"))
        .expect("insert card");
    let d = draft(&store, "draft payload (board-authored)");

    let receipt = process_with_factory(&store, &accept(), &d.draft_id, &ident, "coordinator")
        .expect("process ok");

    assert_eq!(receipt.outcome, Outcome::Accepted);
    assert_eq!(receipt.identity, "thalixinc/herdr-board#42");
    assert_eq!(receipt.revision, "rev-1");
    assert_eq!(receipt.factory, "coordinator");
    // The actor is the proven trust root, never the card or the body.
    assert_eq!(receipt.actor, prove_actor().value);

    // The digest is over the REAL six fields — draft body + card identity/revision
    // + target factory + trust-root actor — not the placeholder promote digest.
    let expected = expected_request(
        &ident,
        "rev-1",
        "coordinator",
        "draft payload (board-authored)",
    );
    assert_eq!(receipt.digest, compute(&expected));
}

#[test]
fn promote_twice_is_refused() {
    set_workspace();
    let store = Store::open_in_memory().expect("open store");
    let ident = identity();
    store
        .insert_card(&card(&ident, "rev-1"))
        .expect("insert card");
    let d = draft(&store, "body");

    process_with_factory(&store, &accept(), &d.draft_id, &ident, "f").expect("first ok");

    let err =
        process_with_factory(&store, &accept(), &d.draft_id, &ident, "f").expect_err("second");
    assert!(matches!(
        err,
        ProcessError::Card(CardError::AlreadyPromoted)
    ));
}

#[test]
fn missing_card_is_refused() {
    set_workspace();
    let store = Store::open_in_memory().expect("open store");
    let d = draft(&store, "body");

    let err = process_with_factory(&store, &accept(), &d.draft_id, &identity(), "f")
        .expect_err("no card");
    assert!(matches!(err, ProcessError::CardNotFound));
}

#[test]
fn transport_failed_leaves_handed_off() {
    set_workspace();
    let store = Store::open_in_memory().expect("open store");
    let ident = identity();
    store
        .insert_card(&card(&ident, "rev-1"))
        .expect("insert card");
    let d = draft(&store, "body");

    let err = process_with_factory(&store, &fail(), &d.draft_id, &ident, "f").expect_err("fail");
    assert!(matches!(
        err,
        ProcessError::Receiver(ReceiverError::Transport(_))
    ));

    // The receipt stays handed-off (outcome unknown), never silently re-run.
    let active = store.list_non_terminal().expect("list");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].outcome, Outcome::HandedOff);
}

#[test]
fn identical_reclick_returns_existing_active_receipt() {
    set_workspace();
    let store = Store::open_in_memory().expect("open store");
    let ident = identity();

    let h1 = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request(&ident, "rev-1", "body"),
    };
    assert!(matches!(
        receive(&store, &h1, &fail()),
        Err(ReceiverError::Transport(_))
    ));

    let active = store.list_non_terminal().expect("list");
    assert_eq!(active.len(), 1);
    let first_id = active[0].receipt_id;

    // Identical re-click (same digest) converges on the existing active receipt.
    let h2 = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request(&ident, "rev-1", "body"),
    };
    match receive(&store, &h2, &fail()) {
        Err(ReceiverError::ActiveAttemptExists { receipt }) => {
            assert_eq!(receipt.receipt_id, first_id);
        }
        other => panic!("expected ActiveAttemptExists, got {other:?}"),
    }
}

#[test]
fn drifted_revision_reclick_converges() {
    set_workspace();
    let store = Store::open_in_memory().expect("open store");
    let ident = identity();

    let h1 = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request(&ident, "rev-1", "body"),
    };
    assert!(matches!(
        receive(&store, &h1, &fail()),
        Err(ReceiverError::Transport(_))
    ));

    // Drifted revision on the SAME in-flight issue: the digest differs, so the
    // digest index does not fire — the identity-scoped index does. The re-click
    // must not dispatch a second attempt.
    let h2 = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request(&ident, "rev-2", "body"),
    };
    assert!(
        receive(&store, &h2, &fail()).is_err(),
        "a drifted re-click must not dispatch a second attempt"
    );

    // Still exactly one active attempt — the issue converged, no double-run.
    assert_eq!(store.list_non_terminal().expect("list").len(), 1);
}

#[test]
fn terminal_then_reclick_is_fresh_request() {
    set_workspace();
    let store = Store::open_in_memory().expect("open store");
    let ident = identity();

    let h1 = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request(&ident, "rev-1", "body"),
    };
    let r1 = receive(&store, &h1, &accept()).expect("first ok");
    assert_eq!(r1.outcome, Outcome::Accepted);

    // A terminal receipt never blocks a fresh request of the same issue.
    let h2 = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: request(&ident, "rev-1", "body"),
    };
    let r2 = receive(&store, &h2, &accept()).expect("fresh ok");
    assert_ne!(r1.receipt_id, r2.receipt_id);
    assert_eq!(r2.outcome, Outcome::Accepted);
}

#[test]
fn forged_actor_is_refused() {
    set_workspace();
    let store = Store::open_in_memory().expect("open store");
    let ident = identity();

    let mut req = request(&ident, "rev-1", "body");
    req.actor = "forged-actor".to_string();

    let h = Handoff {
        handoff_id: HandoffId::new_v4(),
        request: req,
    };
    assert!(matches!(
        receive(&store, &h, &accept()),
        Err(ReceiverError::ActorMismatch { .. })
    ));
}
