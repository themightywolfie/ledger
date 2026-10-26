//! The SQLite store adapter: journals, their projections and the FTS5 recall index in one file.
//!
//! The `entries` table is the source of truth. `claims` and `claims_fts` are projections, written
//! in the same transaction as the entries they come from (ADR-0010).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use ledgermem_core::{
    Claim, Commit, JournalHead, Projection, Scope, Store, StoreError, StoredEntry, genesis_hash,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The schema version this build creates and opens, kept in SQLite's `user_version`.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
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
        content_hash TEXT NOT NULL,
        prev_hash    TEXT NOT NULL,
        entry_hash   TEXT NOT NULL,
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

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Opens or creates the database file at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(StoreError::new)?;
        conn.pragma_update(None, "journal_mode", "wal")
            .map_err(StoreError::new)?;
        // Other processes may hold the write lock: several agents can share one database.
        conn.busy_timeout(BUSY_TIMEOUT).map_err(StoreError::new)?;
        Self::init(conn)
    }

    /// A private database that lives only as long as the store.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory().map_err(StoreError::new)?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(StoreError::new)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(StoreError::new)?;
        match version {
            SCHEMA_VERSION => {}
            0 if has_journal(&conn)? => {
                return Err(StoreError::new(
                    "this database was made by a pre-release build of ledgermem whose journals \
                     have no hash chains; move it aside, and ledgermem will create a new one",
                ));
            }
            0 => {
                conn.execute_batch(SCHEMA).map_err(StoreError::new)?;
                conn.pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(StoreError::new)?;
            }
            newer => {
                return Err(StoreError::new(format!(
                    "this database uses schema version {newer}, newer than this ledgermem \
                     understands ({SCHEMA_VERSION}); upgrade ledgermem to open it"
                )));
            }
        }
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

    fn commit(&self, tenant: &str, commit: Commit) -> Result<(), StoreError> {
        let mut conn = self.conn()?;
        // Take the write lock before reading the scope's last entry, so a writer in another
        // process can't number or chain the same entry.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::new)?;
        for entry in commit.entries {
            let scope = entry.scope.to_string();
            let last: Option<(i64, String)> = tx
                .query_row(
                    "SELECT seq, entry_hash FROM entries WHERE tenant = ?1 AND scope = ?2
                     ORDER BY seq DESC LIMIT 1",
                    params![tenant, scope],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(StoreError::new)?;
            let (seq, prev_hash) = match last {
                Some((seq, entry_hash)) => (seq + 1, entry_hash),
                None => (1, genesis_hash(&scope)),
            };
            // Store exactly what the hashes commit to.
            let entry = entry.seal(u64::try_from(seq).map_err(StoreError::new)?, prev_hash);
            tx.execute(
                "INSERT INTO entries (tenant, scope, seq, id, kind, actor, channel, recorded_at,
                                      content, content_hash, prev_hash, entry_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    tenant,
                    entry.scope,
                    seq,
                    entry.id,
                    entry.kind,
                    entry.actor,
                    entry.channel,
                    entry.recorded_at,
                    entry.content,
                    entry.content_hash,
                    entry.prev_hash,
                    entry.entry_hash,
                ],
            )
            .map_err(StoreError::new)?;
        }
        for projection in commit.projections {
            apply_projection(&tx, tenant, projection)?;
        }
        tx.commit().map_err(StoreError::new)
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
                 ORDER BY bm25(claims_fts), c.id
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

    fn journal(&self, tenant: &str, scope: &str) -> Result<Vec<StoredEntry>, StoreError> {
        let conn = self.conn()?;
        let mut statement = conn
            .prepare(
                "SELECT id, seq, kind, actor, channel, recorded_at, content,
                        content_hash, prev_hash, entry_hash
                 FROM entries WHERE tenant = ?1 AND scope = ?2 ORDER BY seq",
            )
            .map_err(StoreError::new)?;
        let rows = statement
            .query_map(params![tenant, scope], |row| {
                Ok(StoredEntry {
                    id: row.get(0)?,
                    scope: scope.to_owned(),
                    // A negative number can only be tampering; verification reports it as a
                    // missing entry.
                    seq: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                    kind: row.get(2)?,
                    actor: row.get(3)?,
                    channel: row.get(4)?,
                    recorded_at: row.get(5)?,
                    content: row.get(6)?,
                    content_hash: row.get(7)?,
                    prev_hash: row.get(8)?,
                    entry_hash: row.get(9)?,
                })
            })
            .map_err(StoreError::new)?;
        rows.map(|entry| entry.map_err(StoreError::new)).collect()
    }

    fn scopes(&self, tenant: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.conn()?;
        let mut statement = conn
            .prepare("SELECT DISTINCT scope FROM entries WHERE tenant = ?1 ORDER BY scope")
            .map_err(StoreError::new)?;
        let rows = statement
            .query_map([tenant], |row| row.get(0))
            .map_err(StoreError::new)?;
        rows.map(|scope| scope.map_err(StoreError::new)).collect()
    }

    fn replace_projections(
        &self,
        tenant: &str,
        heads: &[JournalHead],
        projections: Vec<Projection>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn()?;
        // Holding the write lock, no entry can land between this check and the commit.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::new)?;
        if journal_heads(&tx, tenant)? != heads {
            return Ok(false);
        }
        tx.execute(
            "DELETE FROM claims_fts WHERE rowid IN (SELECT rowid FROM claims WHERE tenant = ?1)",
            [tenant],
        )
        .map_err(StoreError::new)?;
        tx.execute("DELETE FROM claims WHERE tenant = ?1", [tenant])
            .map_err(StoreError::new)?;
        for projection in projections {
            apply_projection(&tx, tenant, projection)?;
        }
        tx.commit().map_err(StoreError::new)?;
        Ok(true)
    }
}

/// Where each of the tenant's journals ends now, in scope order.
fn journal_heads(tx: &Transaction<'_>, tenant: &str) -> Result<Vec<JournalHead>, StoreError> {
    let mut statement = tx
        .prepare(
            "SELECT scope, MAX(seq) FROM entries WHERE tenant = ?1 GROUP BY scope ORDER BY scope",
        )
        .map_err(StoreError::new)?;
    let rows = statement
        .query_map([tenant], |row| {
            Ok(JournalHead {
                scope: row.get(0)?,
                seq: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
            })
        })
        .map_err(StoreError::new)?;
    rows.map(|head| head.map_err(StoreError::new)).collect()
}

/// Whether the database already has a journal table, however old.
fn has_journal(conn: &Connection) -> Result<bool, StoreError> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'entries')",
        [],
        |row| row.get(0),
    )
    .map_err(StoreError::new)
}

fn apply_projection(
    tx: &Transaction<'_>,
    tenant: &str,
    projection: Projection,
) -> Result<(), StoreError> {
    match projection {
        Projection::PutClaim(claim) => put_claim(tx, tenant, &claim),
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
