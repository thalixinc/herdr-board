//! Actor provenance: the board proves *who* asked, never trusting a payload
//! claim. The trust root is herdr's injected workspace identity, falling back
//! to the OS user.

/// A proven actor identity — the trust-root value captured at action time,
/// never read from the request body or any persisted untrusted field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Actor(pub String);

impl AsRef<str> for Actor {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The channel that produced the trust-root value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActorSource {
    /// `HERDR_WORKSPACE_ID` — the parent herdr process injected it.
    HerdrWorkspace,
    /// OS user (`USER`/`USERNAME`/`LOGNAME`) — fallback when herdr injected no
    /// workspace id.
    OsUser,
}

impl ActorSource {
    /// The canonical provenance channel string recorded on every receipt.
    pub fn as_str(&self) -> &'static str {
        match self {
            ActorSource::HerdrWorkspace => "herdr-workspace-identity",
            ActorSource::OsUser => "os-user",
        }
    }
}

impl std::fmt::Display for ActorSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The trust root: the process context herdr injected (or the OS user), plus
/// which channel supplied it. This is the sole source of actor identity; it is
/// set by the parent process, not by any card/prompt/SQLite content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRoot {
    pub value: String,
    pub source: ActorSource,
}

impl TrustRoot {
    /// Capture the trust-root identity from the process environment:
    /// `HERDR_WORKSPACE_ID` first, then the OS user as a fallback.
    pub fn from_env() -> TrustRoot {
        if let Some(ws) = non_empty_env("HERDR_WORKSPACE_ID") {
            return TrustRoot {
                value: ws,
                source: ActorSource::HerdrWorkspace,
            };
        }
        if let Some(user) = os_user() {
            return TrustRoot {
                value: user,
                source: ActorSource::OsUser,
            };
        }
        TrustRoot {
            value: "unknown".to_string(),
            source: ActorSource::OsUser,
        }
    }
}

/// Prove the actor: bind the captured trust root to an [`Actor`]. This is a
/// pure derivation — the value is only ever the trust-root identity, never a
/// payload claim.
pub fn prove_actor(trust_root: &TrustRoot) -> Actor {
    Actor(trust_root.value.clone())
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn os_user() -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|key| non_empty_env(key))
}
