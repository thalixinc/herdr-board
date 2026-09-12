//! Actor provenance: who is the board acting as?

use std::env;

/// How the actor identity was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActorSource {
    /// Injected by the parent herdr process via `HERDR_WORKSPACE_ID`.
    HerdrWorkspaceIdentity,
    /// The OS user the board process runs as (`$USER`, then `$USERNAME`/`$LOGNAME`).
    OsUser,
}

impl ActorSource {
    /// The stable wire/DB form recorded on receipts and requests.
    pub fn as_str(&self) -> &'static str {
        match self {
            ActorSource::HerdrWorkspaceIdentity => "herdr-workspace-identity",
            ActorSource::OsUser => "os-user",
        }
    }
}

/// A proven actor: the identity the board is acting as, plus how it was proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// The identity value (workspace id or OS user), never read from a body.
    pub value: String,
    /// The provenance channel that established `value`.
    pub source: ActorSource,
}

/// The board's trust root: the identity captured at "Process with factory".
///
/// Resolution order is fixed: `HERDR_WORKSPACE_ID` (set by the parent herdr
/// process) wins; otherwise the OS user (`$USER`, then `$USERNAME`, then
/// `$LOGNAME`). The value is never read from request bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRoot {
    value: String,
    source: ActorSource,
}

impl TrustRoot {
    /// Resolve the trust root from the process environment.
    pub fn from_env() -> TrustRoot {
        if let Some(workspace) = non_empty_var("HERDR_WORKSPACE_ID") {
            return TrustRoot {
                value: workspace,
                source: ActorSource::HerdrWorkspaceIdentity,
            };
        }
        for key in ["USER", "USERNAME", "LOGNAME"] {
            if let Some(user) = non_empty_var(key) {
                return TrustRoot {
                    value: user,
                    source: ActorSource::OsUser,
                };
            }
        }
        // Degenerate: no identity channel available. A stable sentinel still
        // beats reading an identity out of a request body.
        TrustRoot {
            value: "unknown".to_owned(),
            source: ActorSource::OsUser,
        }
    }

    /// Prove the current actor from this trust root.
    pub fn prove(&self) -> Actor {
        Actor {
            value: self.value.clone(),
            source: self.source,
        }
    }
}

fn non_empty_var(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Prove the board's current actor from the process environment.
///
/// This is the single source of truth for "who is acting". The receiver
/// enforces every handoff's `actor` field against this value — never against
/// request bodies.
pub fn prove_actor() -> Actor {
    TrustRoot::from_env().prove()
}
