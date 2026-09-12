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

mod receipt;

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
