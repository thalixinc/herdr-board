//! Integration: card→issue push against a fake client (PullClient + GitHubClient).

use std::cell::{Cell, RefCell, RefMut};

use herdr_board::{
    apply_push, create_draft, discard_push, publish_draft, push_changes, Candidate, CardField,
    Conflict, CreateIntent, CreateResult, FactoryKind, GitHubClient, Identity, Issue, IssueFull,
    IssuePatch, PendingWrite, PullClient, PushError, RepoIdentity, Store, UpdateResult,
    WriteOutcome, DEFAULT_COLUMN,
};

struct FakeClient {
    issues: RefCell<Vec<IssueFull>>,
    next_number: Cell<u64>,
    update_result: RefCell<UpdateResult>,
    update_calls: Cell<usize>,
}

impl FakeClient {
    fn new() -> Self {
        FakeClient {
            issues: RefCell::new(Vec::new()),
            next_number: Cell::new(1),
            update_result: RefCell::new(UpdateResult::Updated {
                updated_at: "r2".to_owned(),
            }),
            update_calls: Cell::new(0),
        }
    }

    fn set_update(&self, result: UpdateResult) {
        *self.update_result.borrow_mut() = result;
    }

    fn issue_mut(&self, number: u64) -> RefMut<'_, IssueFull> {
        RefMut::map(self.issues.borrow_mut(), |v| {
            v.iter_mut().find(|i| i.number == number).unwrap()
        })
    }
}

impl PullClient for FakeClient {
    fn list_issues(&self, _repo: &RepoIdentity) -> Vec<IssueFull> {
        self.issues.borrow().clone()
    }
    fn fetch_issue(&self, _repo: &RepoIdentity, number: u64) -> Option<IssueFull> {
        self.issues
            .borrow()
            .iter()
            .find(|i| i.number == number)
            .cloned()
    }
}

impl GitHubClient for FakeClient {
    fn create_issue(
        &self,
        _repo: &RepoIdentity,
        intent: &CreateIntent,
        _marker: &str,
    ) -> CreateResult {
        let number = self.next_number.get();
        self.next_number.set(number + 1);
        let labels = serde_json::from_str::<Vec<String>>(&intent.labels).unwrap_or_default();
        self.issues.borrow_mut().push(IssueFull {
            number,
            title: intent.title.clone(),
            body: intent.body.clone(),
            state: "open".to_owned(),
            state_reason: None,
            labels,
            assignee: intent.assignee.clone(),
            milestone: None,
            updated_at: "r1".to_owned(),
            url: format!("https://github.com/o/r/issues/{number}"),
        });
        CreateResult::Created(number)
    }

    fn search_issues(&self, _repo: &RepoIdentity, _query: &str) -> Vec<Candidate> {
        Vec::new()
    }

    fn get_issue(&self, _repo: &RepoIdentity, _number: u64) -> Option<Issue> {
        None
    }

    fn update_issue(
        &self,
        _repo: &RepoIdentity,
        _number: u64,
        _patch: &IssuePatch,
    ) -> UpdateResult {
        self.update_calls.set(self.update_calls.get() + 1);
        self.update_result.borrow().clone()
    }
}

fn repo() -> RepoIdentity {
    RepoIdentity::new("ThalixInc", "herdr-board")
}

fn identity(number: u64) -> Identity {
    Identity::new("thalixinc", "herdr-board", number)
}

fn write_for(card: &herdr_board::Card) -> PendingWrite {
    PendingWrite {
        identity: card.identity.clone(),
        base_revision: card.revision.clone(),
        title: Some("Edited title".to_owned()),
        body: None,
        labels: None,
        assignee: None,
        milestone: None,
        outcome: WriteOutcome::Pending,
    }
}

#[test]
fn publish_draft_links_a_card() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "Fix sync", "body").unwrap();

    let card = publish_draft(
        &store,
        &client,
        &draft,
        &repo(),
        &["bug".to_owned()],
        Some("alice"),
    )
    .unwrap();

    assert_eq!(card.identity, identity(1));
    assert_eq!(card.revision, "r1"); // seeded from the re-fetch
    assert_eq!(card.fields.title, "Fix sync");
    assert_eq!(card.fields.body, "body");
    assert_eq!(card.fields.labels, vec!["bug".to_owned()]);
    assert_eq!(card.fields.assignee.as_deref(), Some("alice"));
    assert_eq!(card.column, DEFAULT_COLUMN);
    assert_eq!(card.factory_kind, FactoryKind::Ordinary);
    assert_eq!(card.conflict, Conflict::None);
}

#[test]
fn edit_records_pending_write_without_github_call() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();

    let write = write_for(&card);
    store.record_write(&write).unwrap();

    let got = store.get_pending_write(&card.identity).unwrap().unwrap();
    assert_eq!(got.title.as_deref(), Some("Edited title"));
    assert_eq!(got.base_revision, "r1");
    assert_eq!(
        client.update_calls.get(),
        0,
        "record_write must not call GitHub"
    );
    // Card untouched by the edit.
    let card = store.get_card(&card.identity).unwrap().unwrap();
    assert_eq!(card.fields.title, "T");
}

#[test]
fn second_edit_merges_and_pins_base_revision() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();

    store.record_write(&write_for(&card)).unwrap();

    // A second edit with a different base_revision: body only.
    store
        .record_write(&PendingWrite {
            identity: card.identity.clone(),
            base_revision: "r-other".to_owned(),
            title: None,
            body: Some("Edited body".to_owned()),
            labels: None,
            assignee: None,
            milestone: None,
            outcome: WriteOutcome::Pending,
        })
        .unwrap();

    let got = store.get_pending_write(&card.identity).unwrap().unwrap();
    assert_eq!(got.title.as_deref(), Some("Edited title")); // merged (kept)
    assert_eq!(got.body.as_deref(), Some("Edited body")); // merged (new)
    assert_eq!(got.base_revision, "r1"); // pinned to the original base
}

#[test]
fn push_equal_writes_and_advances_revision() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();
    store.record_write(&write_for(&card)).unwrap();

    // GitHub unchanged (updated_at "r1" == base_revision "r1").
    let card = push_changes(&store, &client, &card.identity).unwrap();
    assert_eq!(card.fields.title, "Edited title");
    assert_eq!(card.revision, "r2"); // advanced from the PATCH response
    assert_eq!(card.conflict, Conflict::None);
    assert!(store.get_pending_write(&card.identity).unwrap().is_none());
    assert_eq!(client.update_calls.get(), 1);
}

#[test]
fn push_drifted_surfaces_conflict_without_patching() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();
    store.record_write(&write_for(&card)).unwrap();

    // GitHub changes the title and advances revision.
    client.issue_mut(1).title = "GitHub changed".to_owned();
    client.issue_mut(1).updated_at = "r3".to_owned();

    let err = push_changes(&store, &client, &card.identity).unwrap_err();
    match err {
        PushError::Conflict { diffs } => {
            assert_eq!(diffs.len(), 1);
            assert_eq!(diffs[0].field, CardField::Title);
            assert_eq!(diffs[0].old, "GitHub changed");
            assert_eq!(diffs[0].new, "Edited title");
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert_eq!(client.update_calls.get(), 0, "never PATCH on drift");
    assert!(store.get_pending_write(&card.identity).unwrap().is_some());
}

#[test]
fn apply_push_writes_board_values_over_github() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();
    store.record_write(&write_for(&card)).unwrap();

    let card = apply_push(&store, &client, &card.identity).unwrap();
    assert_eq!(card.fields.title, "Edited title");
    assert_eq!(card.revision, "r2");
    assert_eq!(client.update_calls.get(), 1);
    assert!(store.get_pending_write(&card.identity).unwrap().is_none());
}

#[test]
fn discard_push_drops_the_write() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();
    store.record_write(&write_for(&card)).unwrap();

    discard_push(&store, &client, &card.identity).unwrap();
    assert!(store.get_pending_write(&card.identity).unwrap().is_none());
}

#[test]
fn uncertain_update_reattempted_only_on_explicit_push() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();
    store.record_write(&write_for(&card)).unwrap();

    client.set_update(UpdateResult::Uncertain("lost".to_owned()));
    let err = push_changes(&store, &client, &card.identity).unwrap_err();
    assert!(matches!(err, PushError::UpdateUncertain(_)));
    let write = store.get_pending_write(&card.identity).unwrap().unwrap();
    assert_eq!(write.outcome, WriteOutcome::Uncertain);
    assert_eq!(client.update_calls.get(), 1);

    // Re-attempted only by an explicit second push.
    client.set_update(UpdateResult::Updated {
        updated_at: "r2".to_owned(),
    });
    let card = push_changes(&store, &client, &card.identity).unwrap();
    assert_eq!(card.revision, "r2");
    assert!(store.get_pending_write(&card.identity).unwrap().is_none());
}

#[test]
fn push_on_apply_pending_card_is_refused() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();
    store.record_write(&write_for(&card)).unwrap();

    // A VS1 pull conflict lands first.
    store.record_conflict(&card.identity, &[], 0).unwrap();

    let err = push_changes(&store, &client, &card.identity).unwrap_err();
    assert!(matches!(err, PushError::PullConflictPending));
    assert_eq!(client.update_calls.get(), 0);
}

#[test]
fn column_factory_kind_state_never_pushed() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], None).unwrap();
    store.record_write(&write_for(&card)).unwrap();

    let card = push_changes(&store, &client, &card.identity).unwrap();
    // Board-local fields survive; state is never in the pushed patch.
    assert_eq!(card.fields.state, "open");
    assert_eq!(card.column, DEFAULT_COLUMN);
    assert_eq!(card.factory_kind, FactoryKind::Ordinary);
}

#[test]
fn assignee_clear_round_trips_through_finalize() {
    let store = Store::open_in_memory().unwrap();
    let client = FakeClient::new();
    let draft = create_draft(&store, FactoryKind::Ordinary, "T", "B").unwrap();
    let card = publish_draft(&store, &client, &draft, &repo(), &[], Some("alice")).unwrap();
    assert_eq!(card.fields.assignee.as_deref(), Some("alice"));

    // Edit clears the assignee (Some(None)).
    store
        .record_write(&PendingWrite {
            identity: card.identity.clone(),
            base_revision: card.revision.clone(),
            title: None,
            body: None,
            labels: None,
            assignee: Some(None),
            milestone: None,
            outcome: WriteOutcome::Pending,
        })
        .unwrap();

    let card = push_changes(&store, &client, &card.identity).unwrap();
    assert!(
        card.fields.assignee.is_none(),
        "assignee cleared on written"
    );
}
