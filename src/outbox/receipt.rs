//! Receipt, outcome, and request-record types, plus the store operations that
//! enforce dedup and active-attempt uniqueness.

use std::fmt;
use std::str::FromStr;

use rusqlite::{params, Params};
use uuid::Uuid;

use super::{Store, StoreError};
use crate::digest::{Digest, DigestId};

/// A UUID v4 receipt identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReceiptId(Uuid);

impl ReceiptId {
    /// Generate a new UUID v4 receipt id.
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for ReceiptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ReceiptId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// A UUID v4 handoff identifier: the caller's id for one attempt, so the
/// receiver can distinguish an idempotent re-delivery from a new attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HandoffId(Uuid);

impl HandoffId {
    /// Generate a new UUID v4 handoff id.
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for HandoffId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for HandoffId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// The state of a handoff attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// Accepted by the receiver; handoff not yet attempted.
    Pending,
    /// Dispatched to coordinator/planner; outcome not yet known.
    HandedOff,
    /// Coordinator/planner accepted.
    Accepted,
    /// Mismatch (G2) or coordinator/planner refused.
    Refused,
    /// Human cancelled.
    Cancelled,
}

impl Outcome {
    /// The canonical lowercase wire form stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Pending => "pending",
            Outcome::HandedOff => "handed-off",
            Outcome::Accepted => "accepted",
            Outcome::Refused => "refused",
            Outcome::Cancelled => "cancelled",
        }
    }

    /// Whether the outcome is a terminal (decided) state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Outcome::Accepted | Outcome::Refused | Outcome::Cancelled
        )
    }

    /// Whether the outcome is a non-terminal (active) state.
    pub fn is_active(&self) -> bool {
        !self.is_terminal()
    }

    fn from_db(s: &str) -> Result<Self, StoreError> {
        match s {
            "pending" => Ok(Outcome::Pending),
            "handed-off" => Ok(Outcome::HandedOff),
            "accepted" => Ok(Outcome::Accepted),
            "refused" => Ok(Outcome::Refused),
            "cancelled" => Ok(Outcome::Cancelled),
            other => Err(StoreError::InvalidData(format!(
                "unknown outcome {other:?}"
            ))),
        }
    }
}

/// The board's request record: the durable home of the G2 digest plus the
/// canonical fields it covers. `digest_id` and `created_at` are derived/set by
/// the store (from the digest and at insert time, respectively), so this type
/// carries only the fields the caller owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRecord {
    /// The full G2 digest — the idempotency key (unique per input).
    pub digest: Digest,
    /// Canonical `owner/repo#number`.
    pub identity: String,
    /// Opaque revision token (e.g. GitHub `updated_at`), verbatim.
    pub revision: String,
    /// Target factory identifier, verbatim.
    pub factory: String,
    /// Proven actor (trust root), never read from the body.
    pub actor: String,
    /// Provenance channel for `actor` (e.g. `herdr-workspace-identity`).
    pub actor_source: String,
    /// Request body as persisted, verbatim.
    pub body: String,
}

/// A durable record that a specific handoff was received, from whom, and how it
/// resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub receipt_id: ReceiptId,
    pub handoff_id: HandoffId,
    pub request_id: String,
    pub digest: Digest,
    /// The 16-hex display prefix of `digest`, derived from it.
    pub digest_id: DigestId,
    pub actor: String,
    pub actor_source: String,
    pub identity: String,
    pub revision: String,
    pub factory: String,
    pub outcome: Outcome,
    /// The coordinator/planner response, verbatim but untrusted.
    pub external_response: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds; set once a terminal outcome is reached.
    pub finalized_at: Option<i64>,
}

impl Store {
    /// Persist a request and acquire its active-attempt slot by inserting a
    /// `pending` receipt.
    ///
    /// The request is upserted by digest: re-clicking identical input reuses the
    /// existing request row (and its `request_id`); changed input produces a new
    /// digest and therefore a new request row. The receipt insert is what fires
    /// the dedup / active-attempt / handoff-id uniqueness constraints.
    pub fn insert_pending(
        &self,
        request: &RequestRecord,
        handoff_id: &HandoffId,
    ) -> Result<Receipt, StoreError> {
        let receipt_id = ReceiptId::new_v4();
        let now = now_unix();

        let digest_bytes: &[u8] = request.digest.as_bytes();
        let digest_id_hex = request.digest.digest_id().to_hex();

        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO requests \
             (request_id, digest, digest_id, identity, revision, factory, actor, actor_source, body, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(digest) DO NOTHING",
            params![
                Uuid::new_v4().to_string(),
                digest_bytes,
                digest_id_hex,
                request.identity,
                request.revision,
                request.factory,
                request.actor,
                request.actor_source,
                request.body,
                now,
            ],
        )?;

        // Resolve the canonical request id (the existing row wins on a digest
        // conflict, so a re-click converges on the same request).
        let request_id: String = tx.query_row(
            "SELECT request_id FROM requests WHERE digest = ?1",
            params![digest_bytes],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO receipts \
             (receipt_id, handoff_id, request_id, digest, digest_id, actor, actor_source, \
              identity, revision, factory, outcome, external_response, created_at, finalized_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', NULL, ?11, NULL)",
            params![
                receipt_id.to_string(),
                handoff_id.to_string(),
                request_id,
                digest_bytes,
                digest_id_hex,
                request.actor,
                request.actor_source,
                request.identity,
                request.revision,
                request.factory,
                now,
            ],
        )
        .map_err(classify_insert_constraint)?;

        tx.commit()?;

        self.receipt_by_id(&receipt_id)?
            .ok_or_else(|| StoreError::InvalidData("receipt vanished after insert".into()))
    }

    /// Transition a non-terminal receipt to `outcome`. Terminal outcomes are
    /// final: a further transition returns [`StoreError::AlreadyFinal`].
    ///
    /// `external_response` is the coordinator/planner ack/refusal, recorded
    /// verbatim; pass [`None`] for non-terminal transitions.
    pub fn transition_to(
        &self,
        receipt_id: &ReceiptId,
        outcome: Outcome,
        external_response: Option<&str>,
    ) -> Result<Receipt, StoreError> {
        let finalized_at = if outcome.is_terminal() {
            Some(now_unix())
        } else {
            None
        };

        let updated = self.conn.execute(
            "UPDATE receipts SET outcome = ?1, external_response = ?2, finalized_at = ?3 \
             WHERE receipt_id = ?4 AND outcome IN ('pending','handed-off')",
            params![
                outcome.as_str(),
                external_response,
                finalized_at,
                receipt_id.to_string()
            ],
        )?;

        if updated == 0 {
            if self.receipt_by_id(receipt_id)?.is_some() {
                return Err(StoreError::AlreadyFinal);
            }
            return Err(StoreError::NotFound);
        }

        self.receipt_by_id(receipt_id)?
            .ok_or_else(|| StoreError::InvalidData("receipt vanished after transition".into()))
    }

    /// Fetch a receipt by its handoff id (idempotent re-delivery lookup).
    pub fn get_by_handoff_id(&self, handoff_id: &HandoffId) -> Result<Option<Receipt>, StoreError> {
        self.query_receipt_opt(
            "SELECT receipt_id, handoff_id, request_id, digest, actor, actor_source, \
             identity, revision, factory, outcome, external_response, created_at, finalized_at \
             FROM receipts WHERE handoff_id = ?1",
            params![handoff_id.to_string()],
        )
    }

    /// Fetch the active (non-terminal) receipt for a digest, if any.
    pub fn get_active_by_digest(&self, digest: &Digest) -> Result<Option<Receipt>, StoreError> {
        self.query_receipt_opt(
            "SELECT receipt_id, handoff_id, request_id, digest, actor, actor_source, \
             identity, revision, factory, outcome, external_response, created_at, finalized_at \
             FROM receipts WHERE digest = ?1 AND outcome IN ('pending','handed-off')",
            params![digest.as_bytes().as_slice()],
        )
    }

    /// Fetch the active (non-terminal) receipt for a request, if any.
    pub fn get_active_by_request(&self, request_id: &str) -> Result<Option<Receipt>, StoreError> {
        self.query_receipt_opt(
            "SELECT receipt_id, handoff_id, request_id, digest, actor, actor_source, \
             identity, revision, factory, outcome, external_response, created_at, finalized_at \
             FROM receipts WHERE request_id = ?1 AND outcome IN ('pending','handed-off')",
            params![request_id],
        )
    }

    /// All non-terminal receipts, oldest first (startup sweep).
    pub fn list_non_terminal(&self) -> Result<Vec<Receipt>, StoreError> {
        self.query_receipts(
            "SELECT receipt_id, handoff_id, request_id, digest, actor, actor_source, \
             identity, revision, factory, outcome, external_response, created_at, finalized_at \
             FROM receipts WHERE outcome IN ('pending','handed-off') ORDER BY created_at ASC",
            (),
        )
    }

    /// Fetch the persisted request record (including `body`) by its id.
    ///
    /// The receiver uses this to reconstruct a [`crate::digest::StoredFields`]
    /// snapshot for drift verification; receipts deliberately do not
    /// denormalize `body`.
    pub fn get_request_by_id(&self, request_id: &str) -> Result<Option<RequestRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT digest, identity, revision, factory, actor, actor_source, body \
             FROM requests WHERE request_id = ?1",
        )?;
        let mut rows = stmt.query(params![request_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(read_request(row)?)),
            None => Ok(None),
        }
    }

    fn receipt_by_id(&self, receipt_id: &ReceiptId) -> Result<Option<Receipt>, StoreError> {
        self.query_receipt_opt(
            "SELECT receipt_id, handoff_id, request_id, digest, actor, actor_source, \
             identity, revision, factory, outcome, external_response, created_at, finalized_at \
             FROM receipts WHERE receipt_id = ?1",
            params![receipt_id.to_string()],
        )
    }

    fn query_receipt_opt<P: Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<Option<Receipt>, StoreError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        match rows.next()? {
            Some(row) => Ok(Some(read_receipt(row)?)),
            None => Ok(None),
        }
    }

    fn query_receipts<P: Params>(&self, sql: &str, params: P) -> Result<Vec<Receipt>, StoreError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(read_receipt(row)?);
        }
        Ok(out)
    }
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn digest_from_blob(blob: Vec<u8>) -> Result<Digest, StoreError> {
    let arr: [u8; 32] = blob.try_into().map_err(|v: Vec<u8>| {
        StoreError::InvalidData(format!("digest blob has {} bytes, expected 32", v.len()))
    })?;
    Ok(Digest(arr))
}

fn read_request(row: &rusqlite::Row<'_>) -> Result<RequestRecord, StoreError> {
    let digest_blob: Vec<u8> = row.get(0)?;
    let identity: String = row.get(1)?;
    let revision: String = row.get(2)?;
    let factory: String = row.get(3)?;
    let actor: String = row.get(4)?;
    let actor_source: String = row.get(5)?;
    let body: String = row.get(6)?;

    Ok(RequestRecord {
        digest: digest_from_blob(digest_blob)?,
        identity,
        revision,
        factory,
        actor,
        actor_source,
        body,
    })
}

fn read_receipt(row: &rusqlite::Row<'_>) -> Result<Receipt, StoreError> {
    let receipt_id: String = row.get(0)?;
    let handoff_id: String = row.get(1)?;
    let request_id: String = row.get(2)?;
    let digest_blob: Vec<u8> = row.get(3)?;
    let actor: String = row.get(4)?;
    let actor_source: String = row.get(5)?;
    let identity: String = row.get(6)?;
    let revision: String = row.get(7)?;
    let factory: String = row.get(8)?;
    let outcome: String = row.get(9)?;
    let external_response: Option<String> = row.get(10)?;
    let created_at: i64 = row.get(11)?;
    let finalized_at: Option<i64> = row.get(12)?;

    let digest = digest_from_blob(digest_blob)?;
    let receipt_id = receipt_id
        .parse::<ReceiptId>()
        .map_err(|e: uuid::Error| StoreError::InvalidData(e.to_string()))?;
    let handoff_id = handoff_id
        .parse::<HandoffId>()
        .map_err(|e: uuid::Error| StoreError::InvalidData(e.to_string()))?;

    Ok(Receipt {
        receipt_id,
        handoff_id,
        request_id,
        digest,
        digest_id: digest.digest_id(),
        actor,
        actor_source,
        identity,
        revision,
        factory,
        outcome: Outcome::from_db(&outcome)?,
        external_response,
        created_at,
        finalized_at,
    })
}

/// Classify a constraint violation from the `receipts` insert into a typed
/// [`StoreError`].
fn classify_insert_constraint(err: rusqlite::Error) -> StoreError {
    if let rusqlite::Error::SqliteFailure(ffi, Some(msg)) = &err {
        if ffi.code == rusqlite::ErrorCode::ConstraintViolation {
            if msg.contains("handoff_id") {
                return StoreError::DuplicateHandoff;
            }
            if msg.contains("digest") || msg.contains("request_id") {
                return StoreError::ActiveAttemptExists;
            }
        }
    }
    StoreError::Sqlite(err)
}
