//! The SQLite store adapter: journals, their projections and the FTS5 recall index in one file.
//!
//! The `entries` table is the source of truth. `claims` and `claims_fts` are projections, written
//! in the same transaction as the entries they come from (ADR-0010).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use ledgermem_core::{Claim, Commit, JournalEntry, Projection, Scope, Store, StoreError};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const SCHEMA: &str = "
    PRAGMA foreign_keys = ON;

    CREATE TABLE IF NOT EXISTS tenants (
        id          TEXT PRIMARY KEY,
        created_at  TEXT NOT NULL
    ) STRICT;

    CREATE TABLE IF NOT EXISTS entries (
        tenant       TEXT NOT NULL REFERENCES tenants (id),
        scope        TEXT NOT NULL,
        seq          INTEGER NOT NULL,
        id           TEXT NOT NULL UNIQUE,
        kind         TEXT NOT NULL,
        actor        TEXT NOT NULL,
        channel      TEXT NOT NULL,
        recorded_at  TEXT NOT NULL,
        content      TEXT,
        PRIMARY KEY (tenant, scope, seq)
    ) STRICT;

    CREATE TABLE IF NOT EXISTS claims (
        id         TEXT NOT NULL UNIQUE,
        tenant     TEXT NOT NULL REFERENCES tenants (id),
        scope      TEXT NOT NULL,
        subject    TEXT NOT NULL,
        predicate  TEXT NOT NULL,
        status     TEXT NOT NULL,
        body       TEXT NOT NULL
    ) STRICT;

    CREATE INDEX IF NOT EXISTS claims_by_key ON claims (tenant, scope, subject, predicate);

    CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5 (text, subject);
";

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Opens or creates the database file at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(StoreError::new)?;
        conn.pragma_update(None, "journal_mode", "wal")
            .map_err(StoreError::new)?;
        Self::init(conn)
    }

    /// A private database that lives only as long as the store.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory().map_err(StoreError::new)?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch(SCHEMA).map_err(StoreError::new)?;
        Ok(SqliteStore {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.conn
            .lock()
            .map_err(|_| StoreError::new("connection lock poisoned"))
    }
}

impl Store for SqliteStore {
    fn tenant_exists(&self, tenant: &str) -> Result<bool, StoreError> {
        self.conn()?
            .query_row("SELECT 1 FROM tenants WHERE id = ?1", [tenant], |_| Ok(()))
            .optional()
            .map(|found| found.is_some())
            .map_err(StoreError::new)
    }

    fn create_tenant(&self, tenant: &str, created_at: OffsetDateTime) -> Result<(), StoreError> {
        let created_at = created_at.format(&Rfc3339).map_err(StoreError::new)?;
        self.conn()?
            .execute(
                "INSERT INTO tenants (id, created_at) VALUES (?1, ?2)",
                params![tenant, created_at],
            )
            .map(drop)
            .map_err(StoreError::new)
    }

    fn commit(&self, tenant: &str, commit: Commit) -> Result<Vec<JournalEntry>, StoreError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(StoreError::new)?;
        let mut appended = Vec::with_capacity(commit.entries.len());
        for entry in commit.entries {
            let scope = entry.scope.to_string();
            let seq: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM entries WHERE tenant = ?1 AND scope = ?2",
                    params![tenant, scope],
                    |row| row.get(0),
                )
                .map_err(StoreError::new)?;
            let entry = entry.numbered(u64::try_from(seq).map_err(StoreError::new)?);
            let row = to_json(&entry)?;
            tx.execute(
                "INSERT INTO entries (tenant, scope, seq, id, kind, actor, channel, recorded_at, content)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    tenant,
                    scope,
                    seq,
                    entry.id,
                    row["kind"].as_str(),
                    entry.actor,
                    row["channel"].as_str(),
                    row["recorded_at"].as_str(),
                    entry.content.to_string(),
                ],
            )
            .map_err(StoreError::new)?;
            appended.push(entry);
        }
        for projection in commit.projections {
            match projection {
                Projection::PutClaim(claim) => put_claim(&tx, tenant, &claim)?,
            }
        }
        tx.commit().map_err(StoreError::new)?;
        Ok(appended)
    }

    fn search(
        &self,
        tenant: &str,
        scopes: &[Scope],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, StoreError> {
        let Some(fts_query) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let scopes = Value::from(scopes.iter().map(Scope::to_string).collect::<Vec<_>>());
        let conn = self.conn()?;
        let mut statement = conn
            .prepare(
                "SELECT c.body FROM claims_fts f JOIN claims c ON c.rowid = f.rowid
                 WHERE claims_fts MATCH ?1
                   AND c.tenant = ?2
                   AND c.scope IN (SELECT value FROM json_each(?3))
                 ORDER BY bm25(claims_fts)
                 LIMIT ?4",
            )
            .map_err(StoreError::new)?;
        let rows = statement
            .query_map(
                params![fts_query, tenant, scopes.to_string(), limit as i64],
                |row| row.get::<_, String>(0),
            )
            .map_err(StoreError::new)?;
        rows.map(|body| from_json(&body.map_err(StoreError::new)?))
            .collect()
    }

    fn journal(&self, tenant: &str, scope: &Scope) -> Result<Vec<JournalEntry>, StoreError> {
        let conn = self.conn()?;
        let mut statement = conn
            .prepare(
                "SELECT id, seq, kind, actor, channel, recorded_at, content FROM entries
                 WHERE tenant = ?1 AND scope = ?2 ORDER BY seq",
            )
            .map_err(StoreError::new)?;
        let rows = statement
            .query_map(params![tenant, scope.to_string()], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "scope": scope.to_string(),
                    "seq": row.get::<_, i64>(1)?,
                    "kind": row.get::<_, String>(2)?,
                    "actor": row.get::<_, String>(3)?,
                    "channel": row.get::<_, String>(4)?,
                    "recorded_at": row.get::<_, String>(5)?,
                    "content": row.get::<_, Option<String>>(6)?,
                }))
            })
            .map_err(StoreError::new)?;
        rows.map(|row| {
            let mut row = row.map_err(StoreError::new)?;
            row["content"] = match row["content"].as_str() {
                Some(content) => serde_json::from_str(content).map_err(StoreError::new)?,
                None => Value::Null,
            };
            serde_json::from_value(row).map_err(StoreError::new)
        })
        .collect()
    }
}

/// Writes a claim's current state and refreshes its full-text row.
fn put_claim(tx: &Transaction<'_>, tenant: &str, claim: &Claim) -> Result<(), StoreError> {
    let body = to_json(claim)?;
    let rowid: i64 = tx
        .query_row(
            "INSERT INTO claims (id, tenant, scope, subject, predicate, status, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET status = excluded.status, body = excluded.body
             RETURNING rowid",
            params![
                claim.id.as_str(),
                tenant,
                claim.scope.to_string(),
                claim.subject,
                claim.predicate,
                body["status"].as_str(),
                body.to_string(),
            ],
            |row| row.get(0),
        )
        .map_err(StoreError::new)?;
    tx.execute("DELETE FROM claims_fts WHERE rowid = ?1", [rowid])
        .map_err(StoreError::new)?;
    tx.execute(
        "INSERT INTO claims_fts (rowid, text, subject) VALUES (?1, ?2, ?3)",
        params![rowid, claim.text, claim.subject],
    )
    .map_err(StoreError::new)?;
    Ok(())
}

/// Turns free text into an FTS5 query that matches any of its words, so punctuation in a
/// question can't be read as FTS5 syntax.
fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{word}\""))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Value, StoreError> {
    serde_json::to_value(value).map_err(StoreError::new)
}

fn from_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, StoreError> {
    serde_json::from_str(text).map_err(StoreError::new)
}
