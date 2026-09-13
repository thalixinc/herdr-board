//! The card store (VS1): one row per GitHub issue pulled onto the board, keyed
//! by the issue identity `(owner, repo, number)`, plus the visible-conflict
//! state (`apply-pending`) and its per-field diffs.
//!
//! `column` and `factory_kind` are board-local and never overwritten by sync;
//! the seven GitHub-canonical fields live in [`CanonicalFields`].

use std::str::Chars;

use rusqlite::{params, Row};

use super::{Store, StoreError};
use crate::digest::Identity;
use crate::kind::FactoryKind;

/// A pulled card: the issue identity plus the board's copy of its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub identity: Identity,
    pub url: String,
    pub fields: CanonicalFields,
    /// Board-local column; sync never rewrites it.
    pub column: String,
    /// Board-local intent; sync never rewrites it.
    pub factory_kind: FactoryKind,
    /// GitHub `updated_at`, verbatim — the revision gate for conflict detection.
    pub revision: String,
    pub conflict: Conflict,
    /// Unix seconds of the last successful sync of this card.
    pub synced_at: i64,
}

/// The seven GitHub-canonical fields the board mirrors. These — and only these —
/// are compared for drift and accepted on `apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalFields {
    pub title: String,
    pub body: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub milestone: Option<String>,
}

/// The visible-conflict state of a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Conflict {
    /// No conflict; the board's copy matches the fetched issue.
    None,
    /// The issue changed on GitHub; awaiting a human apply/defer.
    ApplyPending,
}

impl Conflict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Conflict::None => "none",
            Conflict::ApplyPending => "apply-pending",
        }
    }

    fn from_db(s: &str) -> Result<Self, StoreError> {
        match s {
            "none" => Ok(Conflict::None),
            "apply-pending" => Ok(Conflict::ApplyPending),
            other => Err(StoreError::InvalidData(format!(
                "unknown conflict {other:?}"
            ))),
        }
    }
}

/// One of the seven shared (GitHub-canonical) fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CardField {
    Title,
    Body,
    State,
    StateReason,
    Labels,
    Assignee,
    Milestone,
}

impl CardField {
    pub fn as_str(&self) -> &'static str {
        match self {
            CardField::Title => "title",
            CardField::Body => "body",
            CardField::State => "state",
            CardField::StateReason => "state-reason",
            CardField::Labels => "labels",
            CardField::Assignee => "assignee",
            CardField::Milestone => "milestone",
        }
    }

    fn from_db(s: &str) -> Result<Self, StoreError> {
        match s {
            "title" => Ok(CardField::Title),
            "body" => Ok(CardField::Body),
            "state" => Ok(CardField::State),
            "state-reason" => Ok(CardField::StateReason),
            "labels" => Ok(CardField::Labels),
            "assignee" => Ok(CardField::Assignee),
            "milestone" => Ok(CardField::Milestone),
            other => Err(StoreError::InvalidData(format!(
                "unknown card field {other:?}"
            ))),
        }
    }
}

/// One field that drifted between the board's copy and the fetched issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardFieldDiff {
    pub field: CardField,
    pub old: String,
    pub new: String,
}

const CARD_COLS: &str = "owner, repo, number, url, title, body, state, state_reason, labels, \
     assignee, milestone, column, factory_kind, revision, conflict, synced_at";

fn card_select(where_tail: &str) -> String {
    format!("SELECT {CARD_COLS} FROM cards WHERE {where_tail}")
}

impl Store {
    /// Insert a card. The issue identity `(owner, repo, number)` is the primary
    /// key: a duplicate insert violates the constraint (idempotent).
    pub fn insert_card(&self, card: &Card) -> Result<(), StoreError> {
        let labels = serialize_labels(&card.fields.labels);
        self.conn.execute(
            "INSERT INTO cards \
             (owner, repo, number, url, title, body, state, state_reason, labels, assignee, \
              milestone, column, factory_kind, revision, conflict, synced_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                card.identity.owner,
                card.identity.repo,
                card.identity.number as i64,
                card.url,
                card.fields.title,
                card.fields.body,
                card.fields.state,
                card.fields.state_reason,
                labels,
                card.fields.assignee,
                card.fields.milestone,
                card.column,
                card.factory_kind.as_str(),
                card.revision,
                card.conflict.as_str(),
                card.synced_at,
            ],
        )?;
        Ok(())
    }

    /// Fetch a card by its issue identity.
    pub fn get_card(&self, identity: &Identity) -> Result<Option<Card>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(&card_select("owner = ?1 AND repo = ?2 AND number = ?3"))?;
        let mut rows = stmt.query(params![
            identity.owner,
            identity.repo,
            identity.number as i64
        ])?;
        match rows.next()? {
            Some(row) => Ok(Some(read_card(row)?)),
            None => Ok(None),
        }
    }

    /// List every card in a repo, ordered by issue number.
    pub fn list_cards(&self, owner: &str, repo: &str) -> Result<Vec<Card>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(&card_select("owner = ?1 AND repo = ?2 ORDER BY number ASC"))?;
        let mut rows = stmt.query(params![owner, repo])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(read_card(row)?);
        }
        Ok(out)
    }

    /// Bump a card's `synced_at` (an unchanged re-sync is a no-op content-wise).
    pub fn touch_card(&self, identity: &Identity, synced_at: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE cards SET synced_at = ?4 WHERE owner = ?1 AND repo = ?2 AND number = ?3",
            params![
                identity.owner,
                identity.repo,
                identity.number as i64,
                synced_at
            ],
        )?;
        Ok(())
    }

    /// Flag a card `apply-pending` and store the drifted-field diffs, atomically
    /// replacing any prior conflict rows for this identity.
    pub fn record_conflict(
        &self,
        identity: &Identity,
        diffs: &[CardFieldDiff],
        detected_at: i64,
    ) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE cards SET conflict = 'apply-pending' \
             WHERE owner = ?1 AND repo = ?2 AND number = ?3",
            params![identity.owner, identity.repo, identity.number as i64],
        )?;
        tx.execute(
            "DELETE FROM card_conflicts WHERE owner = ?1 AND repo = ?2 AND number = ?3",
            params![identity.owner, identity.repo, identity.number as i64],
        )?;
        for diff in diffs {
            tx.execute(
                "INSERT INTO card_conflicts \
                 (owner, repo, number, field, old_value, new_value, detected_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    identity.owner,
                    identity.repo,
                    identity.number as i64,
                    diff.field.as_str(),
                    diff.old,
                    diff.new,
                    detected_at,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Read the drifted-field diffs for a card (empty if none), in the
    /// canonical field order.
    pub fn get_conflict(&self, identity: &Identity) -> Result<Vec<CardFieldDiff>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT field, old_value, new_value FROM card_conflicts \
             WHERE owner = ?1 AND repo = ?2 AND number = ?3",
        )?;
        let mut rows = stmt.query(params![
            identity.owner,
            identity.repo,
            identity.number as i64
        ])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let field: String = row.get(0)?;
            let old: String = row.get(1)?;
            let new: String = row.get(2)?;
            out.push(CardFieldDiff {
                field: CardField::from_db(&field)?,
                old,
                new,
            });
        }
        out.sort_by_key(|d| d.field);
        Ok(out)
    }

    /// Accept the fetched canonical fields: overwrite only the seven shared
    /// fields, set `revision`, and clear the conflict. `column`/`factory_kind`
    /// are left untouched.
    pub fn apply_conflict(
        &self,
        identity: &Identity,
        fields: &CanonicalFields,
        revision: &str,
    ) -> Result<Card, StoreError> {
        let labels = serialize_labels(&fields.labels);
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE cards SET title = ?4, body = ?5, state = ?6, state_reason = ?7, labels = ?8, \
             assignee = ?9, milestone = ?10, revision = ?11, conflict = 'none' \
             WHERE owner = ?1 AND repo = ?2 AND number = ?3",
            params![
                identity.owner,
                identity.repo,
                identity.number as i64,
                fields.title,
                fields.body,
                fields.state,
                fields.state_reason,
                labels,
                fields.assignee,
                fields.milestone,
                revision,
            ],
        )?;
        tx.execute(
            "DELETE FROM card_conflicts WHERE owner = ?1 AND repo = ?2 AND number = ?3",
            params![identity.owner, identity.repo, identity.number as i64],
        )?;
        tx.commit()?;

        self.get_card(identity)?.ok_or(StoreError::NotFound)
    }

    /// Defer a conflict: keep the `apply-pending` flag (re-surfaces next sync).
    /// A no-op state-wise — re-affirms the flag only on an already-conflicted
    /// card so a non-conflicted card is never flagged.
    pub fn defer_conflict(&self, identity: &Identity) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE cards SET conflict = 'apply-pending' \
             WHERE owner = ?1 AND repo = ?2 AND number = ?3 AND conflict = 'apply-pending'",
            params![identity.owner, identity.repo, identity.number as i64],
        )?;
        Ok(())
    }
}

fn read_card(row: &Row<'_>) -> Result<Card, StoreError> {
    let owner: String = row.get(0)?;
    let repo: String = row.get(1)?;
    let number: i64 = row.get(2)?;
    let url: String = row.get(3)?;
    let title: String = row.get(4)?;
    let body: String = row.get(5)?;
    let state: String = row.get(6)?;
    let state_reason: Option<String> = row.get(7)?;
    let labels: String = row.get(8)?;
    let assignee: Option<String> = row.get(9)?;
    let milestone: Option<String> = row.get(10)?;
    let column: String = row.get(11)?;
    let factory_kind: String = row.get(12)?;
    let revision: String = row.get(13)?;
    let conflict: String = row.get(14)?;
    let synced_at: i64 = row.get(15)?;

    let factory_kind = factory_kind
        .parse::<FactoryKind>()
        .map_err(|e: crate::kind::InvalidFactoryKind| StoreError::InvalidData(e.to_string()))?;

    Ok(Card {
        identity: Identity::new(owner, repo, number as u64),
        url,
        fields: CanonicalFields {
            title,
            body,
            state,
            state_reason,
            labels: deserialize_labels(&labels)?,
            assignee,
            milestone,
        },
        column,
        factory_kind,
        revision,
        conflict: Conflict::from_db(&conflict)?,
        synced_at,
    })
}

/// Serialize a label list to the canonical JSON-array string (the same
/// convention as `create_intents.labels`).
fn serialize_labels(labels: &[String]) -> String {
    let mut out = String::with_capacity(labels.len().saturating_mul(8).saturating_add(2));
    out.push('[');
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_string(&mut out, label);
    }
    out.push(']');
    out
}

fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Deserialize the canonical JSON-array string back into a label list.
fn deserialize_labels(input: &str) -> Result<Vec<String>, StoreError> {
    let mut chars = input.trim().chars().peekable();
    if chars.next() != Some('[') {
        return Err(StoreError::InvalidData("labels is not a JSON array".into()));
    }

    let mut labels = Vec::new();
    loop {
        while matches!(chars.peek(), Some(c) if *c == ',' || c.is_whitespace()) {
            chars.next();
        }
        match chars.peek() {
            None => return Err(StoreError::InvalidData("unterminated labels array".into())),
            Some(']') => {
                chars.next();
                break;
            }
            Some('"') => labels.push(parse_json_string(&mut chars)?),
            Some(_) => {
                return Err(StoreError::InvalidData(
                    "expected a string in labels array".into(),
                ))
            }
        }
    }
    Ok(labels)
}

fn parse_json_string(chars: &mut std::iter::Peekable<Chars<'_>>) -> Result<String, StoreError> {
    chars.next(); // consume the opening quote
    let mut out = String::new();
    loop {
        match chars.next() {
            Some('"') => return Ok(out),
            Some('\\') => {
                let esc = chars
                    .next()
                    .ok_or_else(|| StoreError::InvalidData("unterminated escape".into()))?;
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'b' => out.push('\u{0008}'),
                    'f' => out.push('\u{000c}'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        let code = parse_hex4(chars)?;
                        out.push(char::from_u32(code).ok_or_else(|| {
                            StoreError::InvalidData("invalid unicode escape".into())
                        })?);
                    }
                    other => {
                        return Err(StoreError::InvalidData(format!("invalid escape \\{other}")))
                    }
                }
            }
            Some(c) => out.push(c),
            None => return Err(StoreError::InvalidData("unterminated string".into())),
        }
    }
}

fn parse_hex4(chars: &mut std::iter::Peekable<Chars<'_>>) -> Result<u32, StoreError> {
    let mut code = 0u32;
    for _ in 0..4 {
        let c = chars
            .next()
            .ok_or_else(|| StoreError::InvalidData("truncated unicode escape".into()))?;
        code = code * 16
            + c.to_digit(16)
                .ok_or_else(|| StoreError::InvalidData("invalid unicode escape".into()))?;
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::{deserialize_labels, serialize_labels};

    #[test]
    fn labels_round_trip() {
        let cases: Vec<Vec<String>> = vec![
            vec![],
            vec!["bug".to_string()],
            vec!["bug".to_string(), "cf:hold".to_string()],
            vec!["with space".to_string(), "comma,name".to_string()],
            vec!["quote\"here".to_string(), "back\\slash".to_string()],
            vec!["héllo".to_string(), "世界".to_string()],
        ];
        for labels in cases {
            let serialized = serialize_labels(&labels);
            assert_eq!(deserialize_labels(&serialized).unwrap(), labels);
        }
    }

    #[test]
    fn labels_empty_and_whitespace() {
        assert!(deserialize_labels("[]").unwrap().is_empty());
        assert!(deserialize_labels("  [ ]  ").unwrap().is_empty());
    }
}
