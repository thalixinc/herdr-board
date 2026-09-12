//! Create-intent outbox (G4): the durable record of a card→GitHub issue
//! create, with a guarded `pending → issued` transition that is the block-auto-
//! replay guarantee.

use std::fmt;
use std::str::FromStr;

use rusqlite::{params, Row};
use uuid::Uuid;

use super::{Store, StoreError};

/// A UUID v4 create-intent identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IntentId(Uuid);

impl IntentId {
    /// Generate a new UUID v4 intent id.
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for IntentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for IntentId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// The client-generated idempotency marker: a unique opaque token, one per
/// create intent. It is a reconciliation key, **not** a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Marker(Uuid);

impl Marker {
    /// Generate a new UUID v4 marker.
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Marker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Marker {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// The state of a create intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CreateOutcome {
    /// Intent persisted; create not yet sent.
    Pending,
    /// Create sent to GitHub; outcome unknown.
    Issued,
    /// GitHub returned the issue number; card linked.
    Created,
    /// GitHub returned a definite error (nothing was created).
    Failed,
    /// Human cancelled.
    Cancelled,
}

impl CreateOutcome {
    /// The canonical lowercase wire form stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            CreateOutcome::Pending => "pending",
            CreateOutcome::Issued => "issued",
            CreateOutcome::Created => "created",
            CreateOutcome::Failed => "failed",
            CreateOutcome::Cancelled => "cancelled",
        }
    }

    /// Whether the outcome is a terminal (decided) state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            CreateOutcome::Created | CreateOutcome::Failed | CreateOutcome::Cancelled
        )
    }

    fn from_db(s: &str) -> Result<Self, StoreError> {
        match s {
            "pending" => Ok(CreateOutcome::Pending),
            "issued" => Ok(CreateOutcome::Issued),
            "created" => Ok(CreateOutcome::Created),
            "failed" => Ok(CreateOutcome::Failed),
            "cancelled" => Ok(CreateOutcome::Cancelled),
            other => Err(StoreError::InvalidData(format!(
                "unknown create outcome {other:?}"
            ))),
        }
    }
}

/// One row of the create-intent outbox: exactly what issue was to be created,
/// the idempotency marker, and the outcome state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateIntent {
    pub intent_id: IntentId,
    pub marker: Marker,
    /// Canonical `owner/repo`, lowercased.
    pub repo: String,
    pub title: String,
    /// Human body verbatim; the marker comment is appended only at send time.
    pub body: String,
    /// JSON array of labels, verbatim.
    pub labels: String,
    pub assignee: Option<String>,
    pub factory_kind: Option<String>,
    pub outcome: CreateOutcome,
    /// Set only on `created`.
    pub issue_number: Option<u64>,
    /// Set only on `failed` — the definite error, for reconciliation display.
    pub reason: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds; set on `pending → issued`.
    pub issued_at: Option<i64>,
    /// Unix seconds; set on a terminal outcome.
    pub finalized_at: Option<i64>,
}

impl CreateIntent {
    /// Construct a fresh intent with a new `intent_id` and `marker`, in the
    /// `pending` state. `assignee`/`factory_kind` default to `None` and may be
    /// set on the returned struct before [`Store::insert_intent`].
    pub fn new(
        marker: Marker,
        repo: String,
        title: String,
        body: String,
        labels: String,
    ) -> CreateIntent {
        CreateIntent {
            intent_id: IntentId::new_v4(),
            marker,
            repo,
            title,
            body,
            labels,
            assignee: None,
            factory_kind: None,
            outcome: CreateOutcome::Pending,
            issue_number: None,
            reason: None,
            created_at: 0,
            issued_at: None,
            finalized_at: None,
        }
    }
}

impl Store {
    /// Persist a create intent in the `pending` state. The `marker` is unique:
    /// re-inserting an existing marker returns [`StoreError::DuplicateMarker`].
    pub fn insert_intent(&self, intent: &CreateIntent) -> Result<CreateIntent, StoreError> {
        let now = now_unix();
        self.conn
            .execute(
                "INSERT INTO create_intents \
                 (intent_id, marker, repo, title, body, labels, assignee, factory_kind, \
                  outcome, issue_number, reason, created_at, issued_at, finalized_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', NULL, NULL, ?9, NULL, NULL)",
                params![
                    intent.intent_id.to_string(),
                    intent.marker.to_string(),
                    intent.repo,
                    intent.title,
                    intent.body,
                    intent.labels,
                    intent.assignee,
                    intent.factory_kind,
                    now,
                ],
            )
            .map_err(classify_intent_insert_constraint)?;

        self.intent_by_id(&intent.intent_id)?
            .ok_or_else(|| StoreError::InvalidData("intent vanished after insert".into()))
    }

    /// Transition `pending → issued`. This is a guarded compare-and-set: the
    /// create may be sent at most once, so a second `mark_issued` (or one on a
    /// terminal intent) returns [`StoreError::CreateIntentNotPending`].
    pub fn mark_issued(&self, intent_id: &IntentId) -> Result<CreateIntent, StoreError> {
        let now = now_unix();
        let updated = self.conn.execute(
            "UPDATE create_intents SET outcome = 'issued', issued_at = ?2 \
             WHERE intent_id = ?1 AND outcome = 'pending'",
            params![intent_id.to_string(), now],
        )?;
        if updated == 0 {
            return Err(self.intent_transition_error(intent_id));
        }
        self.intent_by_id(intent_id)?.ok_or(StoreError::NotFound)
    }

    /// Transition `issued → created`, recording the returned issue number. The
    /// board adopts the existing issue; it never re-creates.
    pub fn mark_created(
        &self,
        intent_id: &IntentId,
        issue_number: u64,
    ) -> Result<CreateIntent, StoreError> {
        let now = now_unix();
        let updated = self.conn.execute(
            "UPDATE create_intents SET outcome = 'created', issue_number = ?2, finalized_at = ?3 \
             WHERE intent_id = ?1 AND outcome = 'issued'",
            params![intent_id.to_string(), issue_number as i64, now],
        )?;
        if updated == 0 {
            return Err(self.intent_transition_error(intent_id));
        }
        self.intent_by_id(intent_id)?.ok_or(StoreError::NotFound)
    }

    /// Transition `issued → failed`, recording the definite error reason.
    pub fn mark_failed(
        &self,
        intent_id: &IntentId,
        reason: &str,
    ) -> Result<CreateIntent, StoreError> {
        let now = now_unix();
        let updated = self.conn.execute(
            "UPDATE create_intents SET outcome = 'failed', reason = ?2, finalized_at = ?3 \
             WHERE intent_id = ?1 AND outcome = 'issued'",
            params![intent_id.to_string(), reason, now],
        )?;
        if updated == 0 {
            return Err(self.intent_transition_error(intent_id));
        }
        self.intent_by_id(intent_id)?.ok_or(StoreError::NotFound)
    }

    /// Transition `pending`/`issued → cancelled`. Cancel only stops tracking;
    /// it never deletes anything on GitHub.
    pub fn mark_cancelled(&self, intent_id: &IntentId) -> Result<CreateIntent, StoreError> {
        let now = now_unix();
        let updated = self.conn.execute(
            "UPDATE create_intents SET outcome = 'cancelled', finalized_at = ?2 \
             WHERE intent_id = ?1 AND outcome IN ('pending','issued')",
            params![intent_id.to_string(), now],
        )?;
        if updated == 0 {
            return Err(self.intent_transition_error(intent_id));
        }
        self.intent_by_id(intent_id)?.ok_or(StoreError::NotFound)
    }

    /// All `issued` intents, oldest-first — the startup-sweep "awaiting link"
    /// population.
    pub fn list_issued(&self) -> Result<Vec<CreateIntent>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(&intent_select("outcome = 'issued' ORDER BY issued_at ASC"))?;
        let mut rows = stmt.query(())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(read_intent(row)?);
        }
        Ok(out)
    }

    /// Look up an intent by its idempotency marker (reconciliation).
    pub fn get_by_marker(&self, marker: &Marker) -> Result<Option<CreateIntent>, StoreError> {
        let mut stmt = self.conn.prepare(&intent_select("marker = ?1"))?;
        let mut rows = stmt.query(params![marker.to_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(read_intent(row)?)),
            None => Ok(None),
        }
    }

    fn intent_by_id(&self, intent_id: &IntentId) -> Result<Option<CreateIntent>, StoreError> {
        let mut stmt = self.conn.prepare(&intent_select("intent_id = ?1"))?;
        let mut rows = stmt.query(params![intent_id.to_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(read_intent(row)?)),
            None => Ok(None),
        }
    }

    fn intent_transition_error(&self, intent_id: &IntentId) -> StoreError {
        match self.intent_by_id(intent_id) {
            Ok(Some(_)) => StoreError::CreateIntentNotPending,
            Ok(None) => StoreError::NotFound,
            Err(e) => e,
        }
    }
}

const INTENT_COLS: &str = "intent_id, marker, repo, title, body, labels, assignee, factory_kind, \
     outcome, issue_number, reason, created_at, issued_at, finalized_at";

fn intent_select(where_tail: &str) -> String {
    format!("SELECT {INTENT_COLS} FROM create_intents WHERE {where_tail}")
}

fn read_intent(row: &Row<'_>) -> Result<CreateIntent, StoreError> {
    let intent_id: String = row.get(0)?;
    let marker: String = row.get(1)?;
    let repo: String = row.get(2)?;
    let title: String = row.get(3)?;
    let body: String = row.get(4)?;
    let labels: String = row.get(5)?;
    let assignee: Option<String> = row.get(6)?;
    let factory_kind: Option<String> = row.get(7)?;
    let outcome: String = row.get(8)?;
    let issue_number: Option<i64> = row.get(9)?;
    let reason: Option<String> = row.get(10)?;
    let created_at: i64 = row.get(11)?;
    let issued_at: Option<i64> = row.get(12)?;
    let finalized_at: Option<i64> = row.get(13)?;

    let intent_id = intent_id
        .parse::<IntentId>()
        .map_err(|e: uuid::Error| StoreError::InvalidData(e.to_string()))?;
    let marker = marker
        .parse::<Marker>()
        .map_err(|e: uuid::Error| StoreError::InvalidData(e.to_string()))?;

    Ok(CreateIntent {
        intent_id,
        marker,
        repo,
        title,
        body,
        labels,
        assignee,
        factory_kind,
        outcome: CreateOutcome::from_db(&outcome)?,
        issue_number: issue_number.map(|n| n as u64),
        reason,
        created_at,
        issued_at,
        finalized_at,
    })
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Classify a constraint violation from the `create_intents` insert.
fn classify_intent_insert_constraint(err: rusqlite::Error) -> StoreError {
    if let rusqlite::Error::SqliteFailure(ffi, Some(msg)) = &err {
        if ffi.code == rusqlite::ErrorCode::ConstraintViolation && msg.contains("marker") {
            return StoreError::DuplicateMarker;
        }
    }
    StoreError::Sqlite(err)
}
