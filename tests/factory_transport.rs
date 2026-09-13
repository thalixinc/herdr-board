//! Integration: `process_with_factory` + `reconfirm` wired to the real
//! `herdr-axi send` transport, with a stub `herdr-axi` executable standing in
//! for the adapter.

mod common;

use herdr_board::{
    create_draft, herdr_axi_send_with_bin, process_with_factory, prove_actor, receive, reconfirm,
    AxiTarget, CanonicalFields, CanonicalRequest, Card, CfQueueContract, CfSubmission, Conflict,
    FactoryKind, Handoff, HandoffId, Identity, Outcome, ProcessError, RealHandoffTransport,
    ReceiverError, Store,
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

/// The real transport wired to a stub `herdr-axi` binary path.
fn axi_transport(bin: &str) -> RealHandoffTransport<impl Fn(&CfQueueContract) -> CfSubmission> {
    let target = AxiTarget::new("herdr-board", "coordinator", None);
    let bin = bin.to_owned();
    RealHandoffTransport::new(None, move |c| {
        herdr_axi_send_with_bin(bin.as_str(), &target, c)
    })
}

#[test]
fn submitted_disposition_finalizes_accepted() {
    set_workspace();
    let dir = common::scratch_dir("factory-submitted");
    let capture = dir.join("capture.jsonl");
    let bin = common::fake_herdr_axi(&dir, "submitted", &capture);

    let store = Store::open_in_memory().expect("open store");
    let ident = identity();
    store
        .insert_card(&card(&ident, "rev-1"))
        .expect("insert card");
    let d = draft(&store, "draft payload (board-authored)");

    let receipt = process_with_factory(
        &store,
        &axi_transport(bin.to_str().unwrap()),
        &d.draft_id,
        &ident,
        "coordinator",
    )
    .expect("process ok");

    assert_eq!(receipt.outcome, Outcome::Accepted);
    assert_eq!(receipt.external_response.as_deref(), Some("submitted"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn not_submitted_disposition_leaves_handed_off() {
    set_workspace();
    let dir = common::scratch_dir("factory-not-submitted");
    let capture = dir.join("capture.jsonl");
    let bin = common::fake_herdr_axi(&dir, "not-submitted", &capture);

    let store = Store::open_in_memory().expect("open store");
    let ident = identity();
    store
        .insert_card(&card(&ident, "rev-1"))
        .expect("insert card");
    let d = draft(&store, "draft payload (board-authored)");

    let err = process_with_factory(
        &store,
        &axi_transport(bin.to_str().unwrap()),
        &d.draft_id,
        &ident,
        "coordinator",
    )
    .unwrap_err();

    assert!(matches!(
        err,
        ProcessError::Receiver(ReceiverError::Transport(_))
    ));
    // The receipt stays handed-off (outcome unknown, retryable) — never accepted.
    let non_terminal = store.list_non_terminal().expect("list non-terminal");
    assert_eq!(non_terminal.len(), 1);
    assert_eq!(non_terminal[0].outcome, Outcome::HandedOff);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_disposition_leaves_handed_off() {
    set_workspace();
    let dir = common::scratch_dir("factory-unknown");
    let capture = dir.join("capture.jsonl");
    let bin = common::fake_herdr_axi(&dir, "unknown", &capture);

    let store = Store::open_in_memory().expect("open store");
    let ident = identity();
    store
        .insert_card(&card(&ident, "rev-1"))
        .expect("insert card");
    let d = draft(&store, "draft payload (board-authored)");

    let err = process_with_factory(
        &store,
        &axi_transport(bin.to_str().unwrap()),
        &d.draft_id,
        &ident,
        "coordinator",
    )
    .unwrap_err();

    assert!(matches!(
        err,
        ProcessError::Receiver(ReceiverError::Transport(_))
    ));
    let non_terminal = store.list_non_terminal().expect("list non-terminal");
    assert_eq!(non_terminal.len(), 1);
    assert_eq!(non_terminal[0].outcome, Outcome::HandedOff);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reconfirm_redispatches_same_message_id() {
    set_workspace();
    let dir_a = common::scratch_dir("factory-reconfirm-a");
    let dir_b = common::scratch_dir("factory-reconfirm-b");
    let capture = dir_a.join("capture.jsonl");
    let bin_busy = common::fake_herdr_axi(&dir_a, "not-submitted", &capture);
    let bin_idle = common::fake_herdr_axi(&dir_b, "submitted", &capture);

    let store = Store::open_in_memory().expect("open store");
    let actor = prove_actor();
    let request = CanonicalRequest {
        factory_kind: FactoryKind::FactoryRequest,
        identity: identity(),
        revision: "rev-1".to_string(),
        factory: "coordinator".to_string(),
        actor: actor.value,
        body: "build the board".to_string(),
    };
    let handoff = Handoff {
        handoff_id: HandoffId::new_v4(),
        request,
    };

    // First attempt: coordinator busy → not-submitted → handed-off.
    let err = receive(&store, &handoff, &axi_transport(bin_busy.to_str().unwrap())).unwrap_err();
    assert!(matches!(err, ReceiverError::Transport(_)));
    let handed_off = store
        .get_by_handoff_id(&handoff.handoff_id)
        .unwrap()
        .unwrap();
    assert_eq!(handed_off.outcome, Outcome::HandedOff);

    // Reconfirm: coordinator idle → submitted → accepted.
    let accepted = reconfirm(
        &store,
        &handed_off.receipt_id,
        &axi_transport(bin_idle.to_str().unwrap()),
    )
    .unwrap();
    assert_eq!(accepted.outcome, Outcome::Accepted);

    // Both dispatches carried the same message_id (idempotent re-dispatch).
    let envelopes = common::captured_envelopes(&capture);
    assert_eq!(envelopes.len(), 2, "one dispatch per attempt");
    let first_id = envelopes[0]["params"]["message_id"]
        .as_str()
        .expect("message_id is a string");
    let second_id = envelopes[1]["params"]["message_id"]
        .as_str()
        .expect("message_id is a string");
    assert_eq!(
        first_id, second_id,
        "reconfirm re-sends the same message_id"
    );

    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}
