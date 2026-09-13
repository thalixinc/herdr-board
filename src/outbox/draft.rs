//! Draft store operations (G5): the raw `drafts` table access, exposed as
//! `pub(crate)` methods the public [`crate::card`] API builds on.

use rusqlite::{params, Row};
use uuid::Uuid;

use super::{Store, StoreError};
use crate::card::Draft;
use crate::kind::FactoryKind;

impl Store {
    /// Insert a draft row with its `factory_kind` in the **same** INSERT — the
    /// kind is a first-class column from the instant the row exists; there is
    /// no post-hoc UPDATE that sets it.
    pub(crate) fn insert_draft(
        &self,
        factory_kind: FactoryKind,
        title: &str,
        body: &str,
    ) -> Result<Draft, StoreError> {
        let draft_id = Uuid::new_v4().to_string();
        let now = now_unix();
        self.conn.execute(
            "INSERT INTO drafts (draft_id, factory_kind, title, body, created_at, published_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![draft_id, factory_kind.as_str(), title, body, now],
        )?;
        self.get_draft_by_id(&draft_id)?
            .ok_or_else(|| StoreError::InvalidData("draft vanished after insert".into()))
    }

    /// Fetch a draft by id.
    pub(crate) fn get_draft_by_id(&self, draft_id: &str) -> Result<Option<Draft>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT draft_id, factory_kind, title, body, created_at, published_at \
             FROM drafts WHERE draft_id = ?1",
        )?;
        let mut rows = stmt.query(params![draft_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(read_draft(row)?)),
            None => Ok(None),
        }
    }

    /// The guarded `ordinary → factory-request` transition: a compare-and-set
    /// UPDATE that returns the number of rows changed (0 ⇒ already promoted or
    /// missing). The only code path that changes a draft's kind.
    pub(crate) fn promote_draft_kind(&self, draft_id: &str) -> Result<usize, StoreError> {
        self.conn
            .execute(
                "UPDATE drafts SET factory_kind = 'factory-request' \
                 WHERE draft_id = ?1 AND factory_kind = 'ordinary'",
                params![draft_id],
            )
            .map_err(StoreError::Sqlite)
    }
}

fn read_draft(row: &Row<'_>) -> Result<Draft, StoreError> {
    let draft_id: String = row.get(0)?;
    let factory_kind: String = row.get(1)?;
    let title: String = row.get(2)?;
    let body: String = row.get(3)?;
    let created_at: i64 = row.get(4)?;
    let published_at: Option<i64> = row.get(5)?;

    let factory_kind = factory_kind
        .parse::<FactoryKind>()
        .map_err(|e: crate::kind::InvalidFactoryKind| StoreError::InvalidData(e.to_string()))?;

    Ok(Draft {
        draft_id,
        factory_kind,
        title,
        body,
        created_at,
        published_at,
    })
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
