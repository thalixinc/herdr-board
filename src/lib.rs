//! herdr-board — greenfield GitHub-issue kanban plugin (see `REQUIREMENTS.md`).
//!
//! This crate is the onboarding scaffold: it establishes the build/test/lint
//! pipeline that `cf onboard --check` requires. The plugin architecture
//! (TUI + daemon + store) and its real behavior land in epic #8, gated on the
//! herdr 0.9.0 plugin/tab/pane compatibility qualification (critic repair G1).

pub mod create;
pub mod digest;
pub mod outbox;
pub mod receiver;

pub use create::{
    cancel, candidates, issue, link, marker_comment, Candidate, CreateError, CreateResult,
    GitHubClient, Issue, Publisher, RepoIdentity,
};
pub use digest::{
    compute, verify, CanonicalRequest, Digest, DigestId, Field, FieldDiff, Identity, Mismatch,
    Refusal, StoredFields, SCHEMA_VERSION,
};
pub use outbox::{
    CreateIntent, CreateOutcome, HandoffId, IntentId, Marker, Outcome, Receipt, ReceiptId,
    RequestRecord, Store,
};
pub use receiver::{
    prove_actor, receive, reconfirm, replay, startup_sweep, status_query, Actor, ActorSource,
    ExternalResponse, Handoff, HandoffResult, HandoffTransport, Receiver, ReceiverError, TrustRoot,
    STALENESS_THRESHOLD,
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
