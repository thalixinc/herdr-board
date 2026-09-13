//! Durable outbox and receipt store.
//!
//! The [`Store`] is the board-local SQLite home for the receiver's durable
//! state: one row per persisted request ([`RequestRecord`]), one row per
//! handoff attempt ([`Receipt`]). It enforces two invariants at the schema
//! level, so they survive restarts and racing writers:
//!
//! - **Dedup** — at most one non-terminal receipt per digest (never
//!   double-accept an in-flight input).
//! - **Active-attempt uniqueness** — at most one non-terminal receipt per
//!   request (repeated clicks converge on one active attempt).
//!
//! A terminal receipt never blocks a fresh attempt of identical input: the
//! uniqueness is scoped to `outcome IN ('pending','handed-off')`, matching G2's
//! no-salt determinism (same input ⇒ same digest).

use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

mod cards;
mod draft;
mod intent;
mod receipt;

pub use cards::{CanonicalFields, Card, CardField, CardFieldDiff, Conflict};
pub use intent::{CreateIntent, CreateOutcome, IntentId, Marker};
pub use receipt::{HandoffId, Outcome, Receipt, ReceiptId, RequestRecord};

/// Migration v1: `requests` + `receipts` + the two partial unique indexes.
const MIGRATION_V1: &str = "
CREATE TABLE requests (
    request_id    TEXT PRIMARY KEY NOT NULL,
    digest        BLOB NOT NULL UNIQUE,
    digest_id     TEXT NOT NULL,
    identity      TEXT NOT NULL,
    revision      TEXT NOT NULL,
    factory       TEXT NOT NULL,
    actor         TEXT NOT NULL,
    actor_source  TEXT NOT NULL,
    body          TEXT NOT NULL,
    created_at    INTEGER NOT NULL
);

CREATE TABLE receipts (
    receipt_id        TEXT PRIMARY KEY NOT NULL,
    handoff_id        TEXT NOT NULL UNIQUE,
    request_id        TEXT NOT NULL REFERENCES requests(request_id),
    digest            BLOB NOT NULL,
    digest_id         TEXT NOT NULL,
    actor             TEXT NOT NULL,
    actor_source      TEXT NOT NULL,
    identity          TEXT NOT NULL,
    revision          TEXT NOT NULL,
    factory           TEXT NOT NULL,
    outcome           TEXT NOT NULL CHECK (outcome IN ('pending','handed-off','accepted','refused','cancelled')),
    external_response TEXT,
    created_at        INTEGER NOT NULL,
    finalized_at      INTEGER
);

CREATE UNIQUE INDEX idx_receipts_active_request
    ON receipts(request_id) WHERE outcome IN ('pending','handed-off');

CREATE UNIQUE INDEX idx_receipts_active_digest
    ON receipts(digest) WHERE outcome IN ('pending','handed-off');
";

/// Migration v2: the create-intent outbox (G4). One row per card→GitHub create
/// intent, keyed by a unique idempotency marker.
const MIGRATION_V2: &str = "
CREATE TABLE create_intents (
    intent_id    TEXT PRIMARY KEY NOT NULL,
    marker       TEXT NOT NULL UNIQUE,
    repo         TEXT NOT NULL,
    title        TEXT NOT NULL,
    body         TEXT NOT NULL,
    labels       TEXT NOT NULL,
    assignee     TEXT,
    factory_kind TEXT,
    outcome      TEXT NOT NULL CHECK (outcome IN ('pending','issued','created','failed','cancelled')),
    issue_number INTEGER,
    reason       TEXT,
    created_at   INTEGER NOT NULL,
    issued_at    INTEGER,
    finalized_at INTEGER
);
";

/// Migration v3 (G5): extend `requests` with the schema version and the
/// factory-kind discriminator.
///
/// `schema_version DEFAULT 1` is the sentinel: pre-G5 rows read back as v1 and
/// verify under the v1 layout via version-on-record. `factory_kind DEFAULT
/// 'factory-request'` is honest — a `CanonicalRequest` only ever came from a
/// factory-request card.
const MIGRATION_V3: &str = "
ALTER TABLE requests ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE requests ADD COLUMN factory_kind TEXT NOT NULL DEFAULT 'factory-request';
";

/// Migration v4 (G5): the board's draft store — one row per card draft, with
/// its `factory_kind` written in the same INSERT (atomic, never a separate
/// later update) — plus the G4 create-intent backfill so no intent row carries
/// a NULL `factory_kind`.
const MIGRATION_V4: &str = "
CREATE TABLE drafts (
    draft_id     TEXT PRIMARY KEY NOT NULL,
    factory_kind TEXT NOT NULL,
    title        TEXT NOT NULL,
    body         TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    published_at INTEGER
);

UPDATE create_intents SET factory_kind = 'ordinary' WHERE factory_kind IS NULL;
";

/// Migration v5 (VS1): the card store — one row per pulled GitHub issue, keyed
/// by the composite identity `(owner, repo, number)`, plus the visible-conflict
/// diffs.
const MIGRATION_V5: &str = "
CREATE TABLE cards (
    owner        TEXT,
    repo         TEXT,
    number       INTEGER,
    url          TEXT,
    title        TEXT,
    body         TEXT,
    state        TEXT,
    state_reason TEXT,
    labels       TEXT,
    assignee     TEXT,
    milestone    TEXT,
    column       TEXT NOT NULL,
    factory_kind TEXT NOT NULL DEFAULT 'ordinary',
    revision     TEXT,
    conflict     TEXT NOT NULL DEFAULT 'none' CHECK (conflict IN ('none','apply-pending')),
    synced_at    INTEGER,
    PRIMARY KEY (owner, repo, number)
);

CREATE TABLE card_conflicts (
    owner       TEXT,
    repo        TEXT,
    number      INTEGER,
    field       TEXT,
    old_value   TEXT,
    new_value   TEXT,
    detected_at INTEGER,
    PRIMARY KEY (owner, repo, number, field)
);
";

/// A single connection to the outbox SQLite database, with the migration and
/// receipt operations.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (or create) the store at `path`, creating parent directories and
    /// applying the schema migration. The database uses WAL journaling and
    /// enforces foreign keys.
    pub fn open(path: impl AsRef<Path>) -> Result<Store, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(StoreError::Io)?;
            }
        }
        let conn = Connection::open(path).map_err(StoreError::Sqlite)?;
        Self::init(conn)
    }

    /// Open an in-memory store (for tests). WAL does not apply to in-memory
    /// databases; the schema and constraints still do.
    pub fn open_in_memory() -> Result<Store, StoreError> {
        let conn = Connection::open_in_memory().map_err(StoreError::Sqlite)?;
        Self::init(conn)
    }

    /// The default on-disk location: `$HERDR_PLUGIN_STATE_DIR/herdr-board.sqlite3`,
    /// falling back to `~/.local/state/herdr-board/herdr-board.sqlite3` when the
    /// environment variable is unset (bare `cargo run`).
    pub fn default_path() -> PathBuf {
        if let Some(state_dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR") {
            return PathBuf::from(state_dir).join("herdr-board.sqlite3");
        }
        match home_dir() {
            Some(home) => home
                .join(".local")
                .join("state")
                .join("herdr-board")
                .join("herdr-board.sqlite3"),
            None => PathBuf::from("herdr-board.sqlite3"),
        }
    }

    fn init(conn: Connection) -> Result<Store, StoreError> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(StoreError::Sqlite)?;
        migrate(&conn)?;
        Ok(Store { conn })
    }
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", (), |row| row.get(0))
        .map_err(StoreError::Sqlite)?;
    if version < 1 {
        let tx = conn.unchecked_transaction().map_err(StoreError::Sqlite)?;
        tx.execute_batch(MIGRATION_V1).map_err(StoreError::Sqlite)?;
        tx.pragma_update(None, "user_version", 1_i64)
            .map_err(StoreError::Sqlite)?;
        tx.commit().map_err(StoreError::Sqlite)?;
    }
    if version < 2 {
        let tx = conn.unchecked_transaction().map_err(StoreError::Sqlite)?;
        tx.execute_batch(MIGRATION_V2).map_err(StoreError::Sqlite)?;
        tx.pragma_update(None, "user_version", 2_i64)
            .map_err(StoreError::Sqlite)?;
        tx.commit().map_err(StoreError::Sqlite)?;
    }
    if version < 3 {
        let tx = conn.unchecked_transaction().map_err(StoreError::Sqlite)?;
        tx.execute_batch(MIGRATION_V3).map_err(StoreError::Sqlite)?;
        tx.pragma_update(None, "user_version", 3_i64)
            .map_err(StoreError::Sqlite)?;
        tx.commit().map_err(StoreError::Sqlite)?;
    }
    if version < 4 {
        let tx = conn.unchecked_transaction().map_err(StoreError::Sqlite)?;
        tx.execute_batch(MIGRATION_V4).map_err(StoreError::Sqlite)?;
        tx.pragma_update(None, "user_version", 4_i64)
            .map_err(StoreError::Sqlite)?;
        tx.commit().map_err(StoreError::Sqlite)?;
    }
    if version < 5 {
        let tx = conn.unchecked_transaction().map_err(StoreError::Sqlite)?;
        tx.execute_batch(MIGRATION_V5).map_err(StoreError::Sqlite)?;
        tx.pragma_update(None, "user_version", 5_i64)
            .map_err(StoreError::Sqlite)?;
        tx.commit().map_err(StoreError::Sqlite)?;
    }
    Ok(())
}

fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    if cfg!(windows) {
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            if !profile.is_empty() {
                return Some(PathBuf::from(profile));
            }
        }
    }
    None
}

/// Errors surfaced by the outbox store.
#[derive(Debug)]
pub enum StoreError {
    /// A non-terminal receipt already exists for this request/digest (the
    /// active-attempt / in-flight-digest partial unique index fired).
    ActiveAttemptExists,
    /// A receipt with this `handoff_id` already exists (idempotent re-delivery).
    DuplicateHandoff,
    /// No receipt with the given id exists.
    NotFound,
    /// The receipt is terminal; terminal outcomes are final.
    AlreadyFinal,
    /// A guarded create-intent transition was attempted from the wrong outcome
    /// (e.g. `mark_issued` on an already-`issued` or terminal intent).
    CreateIntentNotPending,
    /// A create intent with this idempotency marker already exists.
    DuplicateMarker,
    /// Data read back from the database failed to decode (uuid, outcome, digest).
    InvalidData(String),
    /// An underlying SQLite error.
    Sqlite(rusqlite::Error),
    /// An I/O error creating the database file or its directory.
    Io(std::io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::ActiveAttemptExists => {
                write!(f, "an active (non-terminal) attempt already exists")
            }
            StoreError::DuplicateHandoff => {
                write!(f, "a receipt with this handoff id already exists")
            }
            StoreError::NotFound => write!(f, "receipt not found"),
            StoreError::AlreadyFinal => write!(f, "receipt is already terminal"),
            StoreError::CreateIntentNotPending => {
                write!(
                    f,
                    "create intent is not in the expected transitionable state"
                )
            }
            StoreError::DuplicateMarker => {
                write!(f, "a create intent with this marker already exists")
            }
            StoreError::InvalidData(msg) => write!(f, "invalid stored data: {msg}"),
            StoreError::Sqlite(e) => write!(f, "sqlite error: {e}"),
            StoreError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Sqlite(e) => Some(e),
            StoreError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}
