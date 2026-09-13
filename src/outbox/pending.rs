//! The pending-write store (VS2): one row per card→issue update awaiting a
//! push, with the base-revision gate and the tri-state field encoding.
//!
//! The five pushable fields use the same `Option` encoding as the push patch:
//! `title`/`body`/`labels` are single-`Option` (replace-on-set); `assignee`/
//! `milestone` are double-`Option` (`None` = leave, `Some(None)` = clear,
//! `Some(Some(v))` = set). `base_revision` is pinned at first edit and never
//! advances on merge.

use rusqlite::{params, OptionalExtension, Row};

use super::{Store, StoreError};
use crate::digest::Identity;

/// The state of a pending write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteOutcome {
    /// Recorded; not yet pushed.
    Pending,
    /// The push returned a definite error; re-attempted only explicitly.
    Failed,
    /// The push response was lost; outcome unknown.
    Uncertain,
    /// The push succeeded and the card was advanced.
    Written,
    /// Human discarded the write.
    Discarded,
}

impl WriteOutcome {
    /// The canonical lowercase wire form stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteOutcome::Pending => "pending",
            WriteOutcome::Failed => "failed",
            WriteOutcome::Uncertain => "uncertain",
            WriteOutcome::Written => "written",
            WriteOutcome::Discarded => "discarded",
        }
    }

    /// Whether the outcome is terminal (decided, no longer pushable).
    pub fn is_terminal(&self) -> bool {
        matches!(self, WriteOutcome::Written | WriteOutcome::Discarded)
    }

    fn from_db(s: &str) -> Result<Self, StoreError> {
        match s {
            "pending" => Ok(WriteOutcome::Pending),
            "failed" => Ok(WriteOutcome::Failed),
            "uncertain" => Ok(WriteOutcome::Uncertain),
            "written" => Ok(WriteOutcome::Written),
            "discarded" => Ok(WriteOutcome::Discarded),
            other => Err(StoreError::InvalidData(format!(
                "unknown write outcome {other:?}"
            ))),
        }
    }
}

/// One pending card→issue update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWrite {
    pub identity: Identity,
    /// The card's revision at first-edit time, pinned across merges.
    pub base_revision: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub labels: Option<Vec<String>>,
    /// `None` = leave unchanged, `Some(None)` = clear, `Some(Some(v))` = set.
    pub assignee: Option<Option<String>>,
    /// `None` = leave unchanged, `Some(None)` = clear, `Some(Some(v))` = set.
    pub milestone: Option<Option<String>>,
    pub outcome: WriteOutcome,
}

const PENDING_COLS: &str = "owner, repo, number, base_revision, title, body, labels, \
     assignee, milestone, outcome";

/// A pending-write row's (outcome, title, body, labels, assignee, milestone).
type PendingFieldsRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A card row's (title, body, labels, assignee, milestone).
type CardFieldsRow = (String, String, String, Option<String>, Option<String>);

impl Store {
    /// Record (or merge) a pending write for a card.
    ///
    /// If an unresolved write exists, the incoming field values are merged
    /// field-by-field (last-wins) and `base_revision` stays pinned. Otherwise
    /// (no row, or a terminal row) a fresh write is started with the given
    /// `base_revision`.
    pub fn record_write(&self, write: &PendingWrite) -> Result<(), StoreError> {
        let owner = &write.identity.owner;
        let repo = &write.identity.repo;
        let number = write.identity.number as i64;

        let tx = self.conn.unchecked_transaction()?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT outcome FROM pending_writes \
                 WHERE owner = ?1 AND repo = ?2 AND number = ?3",
                params![owner, repo, number],
                |row| row.get(0),
            )
            .optional()?;

        let title = write.title.as_deref();
        let body = write.body.as_deref();
        let labels = write.labels.as_ref().map(|l| serialize_labels(l));
        let assignee = encode_assignee(&write.assignee);
        let milestone = encode_assignee(&write.milestone);

        match existing.as_deref() {
            Some("pending" | "failed" | "uncertain") => {
                tx.execute(
                    "UPDATE pending_writes SET \
                     title = COALESCE(?4, title), body = COALESCE(?5, body), \
                     labels = COALESCE(?6, labels), assignee = COALESCE(?7, assignee), \
                     milestone = COALESCE(?8, milestone), outcome = 'pending', updated_at = NULL \
                     WHERE owner = ?1 AND repo = ?2 AND number = ?3",
                    params![owner, repo, number, title, body, labels, assignee, milestone],
                )?;
            }
            _ => {
                tx.execute(
                    "INSERT INTO pending_writes \
                     (owner, repo, number, base_revision, title, body, labels, assignee, \
                      milestone, outcome, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', NULL) \
                     ON CONFLICT(owner, repo, number) DO UPDATE SET \
                     base_revision = excluded.base_revision, title = excluded.title, \
                     body = excluded.body, labels = excluded.labels, \
                     assignee = excluded.assignee, milestone = excluded.milestone, \
                     outcome = 'pending', updated_at = NULL",
                    params![
                        owner,
                        repo,
                        number,
                        &write.base_revision,
                        title,
                        body,
                        labels,
                        assignee,
                        milestone
                    ],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Fetch the unresolved (non-terminal) write for a card, if any.
    pub fn get_pending_write(
        &self,
        identity: &Identity,
    ) -> Result<Option<PendingWrite>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PENDING_COLS} FROM pending_writes \
             WHERE owner = ?1 AND repo = ?2 AND number = ?3 \
             AND outcome IN ('pending','failed','uncertain')"
        ))?;
        let mut rows = stmt.query(params![
            &identity.owner,
            &identity.repo,
            identity.number as i64
        ])?;
        match rows.next()? {
            Some(row) => Ok(Some(read_pending(row)?)),
            None => Ok(None),
        }
    }

    /// Finalize a non-terminal write.
    ///
    /// On [`WriteOutcome::Written`], the pending fields are merged into the
    /// card (title/body/labels/assignee/milestone only — never
    /// `state`/`state_reason`/`column`/`factory_kind`), the card's `revision`
    /// advances to `updated_at`, and any conflict is cleared.
    pub fn finalize_write(
        &self,
        identity: &Identity,
        outcome: WriteOutcome,
        updated_at: Option<&str>,
    ) -> Result<(), StoreError> {
        let owner = &identity.owner;
        let repo = &identity.repo;
        let number = identity.number as i64;

        let tx = self.conn.unchecked_transaction()?;

        let row: Option<PendingFieldsRow> = tx
            .query_row(
                "SELECT outcome, title, body, labels, assignee, milestone \
                 FROM pending_writes WHERE owner = ?1 AND repo = ?2 AND number = ?3",
                params![owner, repo, number],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()?;

        let Some((outcome_db, title, body, labels, assignee, milestone)) = row else {
            return Err(StoreError::NotFound);
        };
        if WriteOutcome::from_db(&outcome_db)?.is_terminal() {
            return Err(StoreError::AlreadyFinal);
        }

        tx.execute(
            "UPDATE pending_writes SET outcome = ?4, updated_at = ?5 \
             WHERE owner = ?1 AND repo = ?2 AND number = ?3",
            params![owner, repo, number, outcome.as_str(), updated_at],
        )?;

        if outcome == WriteOutcome::Written {
            let (c_title, c_body, c_labels, c_assignee, c_milestone): CardFieldsRow = tx
                .query_row(
                    "SELECT title, body, labels, assignee, milestone FROM cards \
                 WHERE owner = ?1 AND repo = ?2 AND number = ?3",
                    params![owner, repo, number],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )?;

            let new_title = title.unwrap_or(c_title);
            let new_body = body.unwrap_or(c_body);
            let new_labels = labels.unwrap_or(c_labels);
            let new_assignee = merge_assignee(assignee, c_assignee);
            let new_milestone = merge_assignee(milestone, c_milestone);

            tx.execute(
                "UPDATE cards SET title = ?4, body = ?5, labels = ?6, assignee = ?7, \
                 milestone = ?8, revision = ?9, conflict = 'none' \
                 WHERE owner = ?1 AND repo = ?2 AND number = ?3",
                params![
                    owner,
                    repo,
                    number,
                    new_title,
                    new_body,
                    new_labels,
                    new_assignee,
                    new_milestone,
                    updated_at,
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Discard a non-terminal write (terminal `discarded`, kept for history).
    pub fn discard_write(&self, identity: &Identity) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE pending_writes SET outcome = 'discarded', updated_at = NULL \
             WHERE owner = ?1 AND repo = ?2 AND number = ?3 \
             AND outcome IN ('pending','failed','uncertain')",
            params![&identity.owner, &identity.repo, identity.number as i64],
        )?;
        Ok(())
    }
}

fn read_pending(row: &Row<'_>) -> Result<PendingWrite, StoreError> {
    let owner: String = row.get(0)?;
    let repo: String = row.get(1)?;
    let number: i64 = row.get(2)?;
    let base_revision: String = row.get(3)?;
    let title: Option<String> = row.get(4)?;
    let body: Option<String> = row.get(5)?;
    let labels: Option<String> = row.get(6)?;
    let assignee: Option<String> = row.get(7)?;
    let milestone: Option<String> = row.get(8)?;
    let outcome: String = row.get(9)?;

    Ok(PendingWrite {
        identity: Identity::new(owner, repo, number as u64),
        base_revision,
        title,
        body,
        labels: labels.map(|s| deserialize_labels(&s)).transpose()?,
        assignee: decode_assignee(assignee),
        milestone: decode_assignee(milestone),
        outcome: WriteOutcome::from_db(&outcome)?,
    })
}

fn serialize_labels(labels: &[String]) -> String {
    serde_json::to_string(labels).expect("serialize labels")
}

fn deserialize_labels(input: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(input).map_err(|e| StoreError::InvalidData(e.to_string()))
}

fn encode_assignee(value: &Option<Option<String>>) -> Option<String> {
    match value {
        None => None,
        Some(None) => Some(String::new()),
        Some(Some(v)) => Some(v.clone()),
    }
}

fn decode_assignee(value: Option<String>) -> Option<Option<String>> {
    value.map(|s| if s.is_empty() { None } else { Some(s) })
}

fn merge_assignee(pending: Option<String>, card: Option<String>) -> Option<String> {
    match pending {
        None => card,
        Some(s) if s.is_empty() => None,
        Some(s) => Some(s),
    }
}
