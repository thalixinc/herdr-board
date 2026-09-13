//! The real board→coordinator transport: the single translation point from the
//! board's canonical request to the five-field contract the coordinator's queue
//! consumes, plus the three-way outcome mapping. The concrete wire invocation
//! is [`herdr_axi_send`] — a blocking `herdr-axi --request-file` subprocess that
//! pushes the request to the coordinator's terminal via herdr-axi's `send`
//! operation and maps its disposition back onto the board's three-way outcome.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::digest::CanonicalRequest;
use crate::sync::Credentials;

use super::{ExternalResponse, HandoffResult, HandoffTransport};

/// The herdr-axi binary on `PATH` — the machine entry point the board consumes.
pub const HERDR_AXI_BIN: &str = "herdr-axi";

/// The target factory role: the factory's `coordinator` (the `f` action's
/// default target). The coordinator accepts/rejects via its own queue + SDLC
/// state; the board never files the queue item itself.
pub const DEFAULT_FACTORY: &str = "coordinator";

/// The board→coordinator field contract: the five fields the board hands to the
/// coordinator/planner's queue. This aligns on the *field contract* only — it
/// never re-owns the coordinator's request encoding (the concrete wire form
/// belongs to herdr-axi).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CfQueueContract {
    pub identity: String,
    pub revision: String,
    pub factory: String,
    pub actor: String,
    pub body: String,
}

impl CfQueueContract {
    /// Build the contract fields from a canonical request. `factory_kind` is a
    /// board-local discriminator and is not part of the contract; `identity`
    /// is canonicalized, the other four fields are verbatim (never re-encoded).
    pub fn from_request(request: &CanonicalRequest) -> CfQueueContract {
        CfQueueContract {
            identity: request.identity.canonical(),
            revision: request.revision.clone(),
            factory: request.factory.clone(),
            actor: request.actor.clone(),
            body: request.body.clone(),
        }
    }
}

/// The external outcome of a coordinator submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfSubmission {
    /// The coordinator accepted the request (delivery accepted; the
    /// coordinator's own accept/reject resolves later via `status_query`).
    Accepted { response: String },
    /// The coordinator refused the request (a normal decision, not a failure).
    Refused { response: String },
    /// The transport could not deliver or the outcome is unknown; the receipt
    /// stays `handed-off` for later reconciliation.
    Failed { reason: String },
}

/// Board-side resolution of the target herdr-axi station: which project, which
/// role, and (optionally) which state root. Resolution is board-side config —
/// the project/state-dir are read from `HERDR_*` context + the plugin manifest,
/// with a dev fallback constant — never a herdr-axi change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxiTarget {
    project: String,
    role: String,
    state_dir: Option<PathBuf>,
}

impl AxiTarget {
    /// Resolve from the environment: `HERDR_PROJECT`, else the `project` field
    /// of `HERDR_PLUGIN_CONTEXT_JSON`, else the dev fallback constant. The
    /// state dir is `HERDR_STATE_DIR`, else the manifest's `state_dir` field,
    /// else unset (herdr-axi's own default `.herdr-axi/state`).
    pub fn from_env() -> Self {
        let project = non_empty_var("HERDR_PROJECT")
            .or_else(|| manifest_field("project"))
            .unwrap_or_else(|| "herdr-board".to_owned());
        let state_dir = non_empty_var("HERDR_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| manifest_field("state_dir").map(PathBuf::from));
        Self {
            project,
            role: DEFAULT_FACTORY.to_owned(),
            state_dir,
        }
    }

    /// An explicit target (tests, and callers that resolve the config themselves).
    pub fn new(
        project: impl Into<String>,
        role: impl Into<String>,
        state_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            project: project.into(),
            role: role.into(),
            state_dir,
        }
    }

    /// The target project name (herdr-axi `station.run_id`).
    pub fn project(&self) -> &str {
        &self.project
    }

    /// The target role (herdr-axi `station.role_id`).
    pub fn role(&self) -> &str {
        &self.role
    }
}

fn non_empty_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn manifest_field(key: &str) -> Option<String> {
    let json = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok()?;
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    value.get(key).and_then(|v| v.as_str()).map(str::to_owned)
}

/// The concrete wire invocation: push one [`CfQueueContract`] to the coordinator
/// through herdr-axi's `send` operation, blocking synchronously (matching the
/// frozen [`HandoffTransport::handoff`] contract between two committed
/// write-ahead transactions).
pub fn herdr_axi_send(contract: &CfQueueContract) -> CfSubmission {
    herdr_axi_send_with_bin(HERDR_AXI_BIN, &AxiTarget::from_env(), contract)
}

/// The injectable seam: the same wire invocation against an explicit binary and
/// target (mirrors herdr-axi's own `send_with_bin` test seam). [`herdr_axi_send`]
/// is this function with the `PATH` binary and the environment-resolved target.
pub fn herdr_axi_send_with_bin(
    bin: &str,
    target: &AxiTarget,
    contract: &CfQueueContract,
) -> CfSubmission {
    // 1. Serialize the contract as the message body (the board's own form: all
    //    five fields verbatim — never re-encoding the coordinator's form).
    let body = match serde_json::to_string(contract) {
        Ok(body) => body,
        Err(e) => {
            return CfSubmission::Failed {
                reason: format!("serialize contract: {e}"),
            }
        }
    };

    // 2. Build the send envelope: operation = send, station = (project, role),
    //    params.text = body, message_id = the deterministic contract fingerprint
    //    (stable across `reconfirm`, so herdr-axi dedups instead of
    //    double-delivering).
    let envelope = serde_json::json!({
        "schema_version": "1",
        "operation_id": fresh_operation_id(),
        "operation": "send",
        "station": {
            "package_id": "manual",
            "run_id": target.project(),
            "role_id": target.role(),
            "instance_id": format!("{}-{}", target.role(), target.project()),
        },
        "params": {
            "text": body,
            "message_id": message_id(&body),
        },
    });

    // 3. Invoke `herdr-axi --request-file <envelope> --json [--state-dir <dir>]`.
    let response = match invoke(bin, target, &envelope) {
        Ok(response) => response,
        Err(reason) => return CfSubmission::Failed { reason },
    };

    // 4. Map the disposition onto the board's three-way outcome.
    map_disposition(&response)
}

/// A fresh operation id for one envelope (the invocation attribution, not an
/// idempotency key — dedup is the `message_id`).
fn fresh_operation_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("op-{nanos}")
}

/// A stable idempotency key for one contract: the same five fields always yield
/// the same key, so `reconfirm` re-dispatches with the same `message_id`.
fn message_id(serialized_body: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(serialized_body.as_bytes());
    format!("herdr-board:{}", hex_lower(&digest))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// The `herdr-axi --json` response fields the board reads (the rest is ignored).
#[derive(serde::Deserialize)]
struct AxiResponse {
    disposition: String,
    #[serde(default)]
    result: serde_json::Value,
}

/// Write the envelope to a temp file and invoke the binary, returning its parsed
/// response. Any failure (binary absent, non-zero exit, unparsable output) maps
/// to a `Failed` reason.
fn invoke(
    bin: &str,
    target: &AxiTarget,
    envelope: &serde_json::Value,
) -> Result<AxiResponse, String> {
    let path = std::env::temp_dir().join(format!(
        "herdr-board-envelope-{}-{}.json",
        std::process::id(),
        fresh_operation_id()
    ));
    let serialized =
        serde_json::to_vec(envelope).map_err(|e| format!("serialize envelope: {e}"))?;
    std::fs::write(&path, serialized).map_err(|e| format!("write envelope: {e}"))?;

    let mut cmd = Command::new(bin);
    cmd.arg("--request-file").arg(&path).arg("--json");
    if let Some(state_dir) = &target.state_dir {
        cmd.arg("--state-dir").arg(state_dir);
    }

    let out = cmd
        .output()
        .map_err(|e| format!("{bin} unavailable: {e}"))?;

    let _ = std::fs::remove_file(&path);

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let line = stderr.lines().next().unwrap_or_default();
        let reason = if line.is_empty() {
            format!("{bin} exited {}", out.status)
        } else {
            // The first stderr line is herdr-axi's JSON error diagnostic; take a
            // bounded slice so the reason never grows unbounded.
            format!("{bin}: {line}")
        };
        return Err(reason);
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).map_err(|e| format!("parse {bin} response: {e}"))
}

/// Map a `herdr-axi send` disposition onto the board's three-way outcome:
/// `submitted` → `Accepted`; `not-submitted` (busy/blocked/unverified) and
/// `unknown` (crash/timeout after possible dispatch) both → `Failed` — the
/// receipt stays `handed-off`, and `unknown` is never auto-rerun.
fn map_disposition(response: &AxiResponse) -> CfSubmission {
    match response.disposition.as_str() {
        "submitted" => CfSubmission::Accepted {
            response: "submitted".to_owned(),
        },
        "not-submitted" => {
            let detail = response
                .result
                .get("reason")
                .and_then(|r| r.as_str())
                .or_else(|| response.result.get("state").and_then(|s| s.as_str()))
                .unwrap_or("busy");
            CfSubmission::Failed {
                reason: format!("not-submitted: {detail}"),
            }
        }
        "unknown" => {
            let detail = response
                .result
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or("outcome unknown after possible dispatch");
            CfSubmission::Failed {
                reason: format!("unknown: {detail}"),
            }
        }
        other => CfSubmission::Failed {
            reason: format!("unexpected disposition: {other}"),
        },
    }
}

/// The real board→coordinator transport. Holds credentials in memory only
/// (never persisted, never logged). The `send` closure is the injected seam
/// (wired to [`herdr_axi_send`] in production); the transport itself owns the
/// translation and the three-way outcome mapping.
pub struct RealHandoffTransport<F> {
    _credentials: Option<Credentials>,
    send: F,
}

impl<F: Fn(&CfQueueContract) -> CfSubmission> RealHandoffTransport<F> {
    /// Build the transport from in-memory credentials and a submission closure.
    pub fn new(credentials: Option<Credentials>, send: F) -> Self {
        Self {
            _credentials: credentials,
            send,
        }
    }
}

impl<F: Fn(&CfQueueContract) -> CfSubmission> HandoffTransport for RealHandoffTransport<F> {
    fn handoff(&self, request: &CanonicalRequest) -> HandoffResult {
        let contract = CfQueueContract::from_request(request);
        match (self.send)(&contract) {
            CfSubmission::Accepted { response } => {
                HandoffResult::Accepted(ExternalResponse::new(response))
            }
            CfSubmission::Refused { response } => {
                HandoffResult::Refused(ExternalResponse::new(response))
            }
            CfSubmission::Failed { reason } => HandoffResult::Failed(reason),
        }
    }
}
