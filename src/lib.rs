//! herdr-board — greenfield GitHub-issue kanban plugin (see `REQUIREMENTS.md`).
//!
//! This crate is the onboarding scaffold: it establishes the build/test/lint
//! pipeline that `cf onboard --check` requires. The plugin architecture
//! (TUI + daemon + store) and its real behavior land in epic #8, gated on the
//! herdr 0.9.0 plugin/tab/pane compatibility qualification (critic repair G1).

pub mod card;
pub mod create;
pub mod digest;
pub mod factory;
pub mod kind;
pub mod outbox;
pub mod push;
pub mod receiver;
pub mod sync;

pub use card::{create_draft, factory_kind_of, promote_to_factory_request, CardError, Draft};

pub use create::{
    cancel, candidates, issue, link, marker_comment, Candidate, CreateError, CreateResult,
    GitHubClient, Issue, IssuePatch, Publisher, RepoIdentity, UpdateResult,
};
pub use digest::{
    compute, compute_with_version, verify, CanonicalRequest, Digest, DigestId, Field, FieldDiff,
    Identity, Mismatch, Refusal, StoredFields, SCHEMA_VERSION,
};
pub use factory::{process_with_factory, ProcessError};
pub use kind::FactoryKind;
pub use outbox::{
    CanonicalFields, Card, CardField, CardFieldDiff, Conflict, CreateIntent, CreateOutcome,
    HandoffId, IntentId, Marker, Outcome, PendingWrite, Receipt, ReceiptId, RequestRecord, Store,
    WriteOutcome,
};
pub use push::{apply_push, discard_push, publish_draft, push_changes, PushError};
pub use receiver::{
    cancel_receipt, prove_actor, receive, reconfirm, replay, startup_sweep, status_query, Actor,
    ActorSource, CfQueueContract, CfSubmission, ExternalResponse, Handoff, HandoffResult,
    HandoffTransport, RealHandoffTransport, Receiver, ReceiverError, TrustRoot,
    STALENESS_THRESHOLD,
};
pub use sync::{
    apply_changes, defer_changes, sync, Credentials, IssueFull, PullClient, RealGitHubClient,
    SyncError, SyncSummary, DEFAULT_COLUMN,
};

/// The plugin identity, surfaced to the herdr pane metadata.
pub const PLUGIN_NAME: &str = "herdr-board";

/// The `cargo` package version, exposed for the pane's own metadata.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::{version, PLUGIN_NAME};

    #[test]
    fn exposes_stable_identity() {
        assert_eq!(PLUGIN_NAME, "herdr-board");
    }

    #[test]
    fn exposes_semver_version() {
        let v = version();
        let mut parts = v.split('.');
        assert!(parts.next().is_some(), "version has a major component");
        assert!(parts.next().is_some(), "version has a minor component");
        assert!(parts.next().is_some(), "version has a patch component");
        assert!(
            parts.next().is_none(),
            "version is exactly MAJOR.MINOR.PATCH"
        );
    }
}
