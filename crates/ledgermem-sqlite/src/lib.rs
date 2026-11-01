//! The SQLite store adapter: journals, their projections and the FTS5 recall index in one file.
//!
//! The `entries` table is the source of truth. `claims`, `claims_fts` and `policies` are
//! projections, written in the same transaction as the entries they come from (ADR-0010).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ledgermem_core::{
    Claim, ClaimId, Commit, JournalHead, PolicyVersion, Projection, Scope, Store, StoreError,
    StoredEntry, StoredJournal, genesis_hash,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The schema version this build creates and opens, kept in SQLite's `user_version`.
const SCHEMA_VERSION: i64 = 1 + MIGRATIONS.len() as i64;

/// The schema as version 1 created it. [`MIGRATIONS`] bring it up to date.
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

/// Each migration takes the schema up one version: the first from version 1 to 2.
const MIGRATIONS: [&str; 1] = [
    // A projection, so it starts empty: no journal before version 2 holds a policy version.
    "CREATE TABLE policies (
        tenant   TEXT NOT NULL REFERENCES tenants (id),
        scope    TEXT NOT NULL,
        version  TEXT NOT NULL,
        body     TEXT NOT NULL,
        PRIMARY KEY (tenant, scope)
    ) STRICT;",
];

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Opens or creates the database file at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(StoreError::new)?;
        // Other processes may hold a lock: several agents can share one database.
        conn.busy_timeout(BUSY_TIMEOUT).map_err(StoreError::new)?;
        use_wal(&conn)?;
        Self::init(conn)
    }

    /// A private database that lives only as long as the store.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory().map_err(StoreError::new)?)
    }

    fn init(mut conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(StoreError::new)?;
        if schema_version(&conn)? != SCHEMA_VERSION {
            // Decide and create under the write lock, so a process opening the same new
            // database at the same moment waits for the whole schema instead of seeing part.
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(StoreError::new)?;
            match schema_version(&tx)? {
                SCHEMA_VERSION => {}
                0 if has_journal(&tx)? => {
                    return Err(StoreError::new(
                        "this database was made by a pre-release build of ledgermem whose \
                         journals have no hash chains; move it aside, and ledgermem will create \
                         a new one",
                    ));
                }
                version @ 0..SCHEMA_VERSION => {
                    if version == 0 {
                        tx.execute_batch(SCHEMA).map_err(StoreError::new)?;
                    }
                    let applied = usize::try_from(version.max(1) - 1).map_err(StoreError::new)?;
                    for migration in &MIGRATIONS[applied..] {
                        tx.execute_batch(migration).map_err(StoreError::new)?;
                    }
                    tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                        .map_err(StoreError::new)?;
                }
                newer => {
                    return Err(StoreError::new(format!(
                        "this database uses schema version {newer}, newer than this ledgermem \
                         understands ({SCHEMA_VERSION}); upgrade ledgermem to open it"
                    )));
                }
            }
            tx.commit().map_err(StoreError::new)?;
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
        self.commit_at(tenant, &[], commit).map(drop)
    }

    fn commit_at(
        &self,
        tenant: &str,
        heads: &[JournalHead],
        commit: Commit,
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn()?;
        // Take the write lock before reading any journal's end, so a writer in another process
        // can't land an entry between this check and the commit, or number or chain the same
        // entry.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::new)?;
        for head in heads {
            if journal_end(&tx, tenant, &head.scope)? != head.seq {
                return Ok(false);
            }
        }
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
        tx.commit().map_err(StoreError::new)?;
        Ok(true)
    }

    fn claim(
        &self,
        tenant: &str,
        id: &ClaimId,
    ) -> Result<Option<(Claim, JournalHead)>, StoreError> {
        let conn = self.conn()?;
        // One statement reads one snapshot, so the claim and its journal's end agree.
        let found = conn
            .query_row(
                "SELECT c.body, c.scope,
                        (SELECT COALESCE(MAX(e.seq), 0) FROM entries e
                         WHERE e.tenant = c.tenant AND e.scope = c.scope)
                 FROM claims c WHERE c.tenant = ?1 AND c.id = ?2",
                params![tenant, id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(StoreError::new)?;
        let Some((body, scope, seq)) = found else {
            return Ok(None);
        };
        let head = JournalHead {
            scope,
            seq: u64::try_from(seq).map_err(StoreError::new)?,
        };
        Ok(Some((from_json(&body)?, head)))
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
                   AND c.status NOT IN ('quarantined', 'retracted')
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

    fn policies(&self, tenant: &str, scopes: &[Scope]) -> Result<Vec<PolicyVersion>, StoreError> {
        let scopes: Vec<String> = scopes.iter().map(Scope::to_string).collect();
        let conn = self.conn()?;
        // One statement reads one snapshot, so no apply lands between scopes.
        let mut statement = conn
            .prepare(
                "SELECT scope, body FROM policies
                 WHERE tenant = ?1 AND scope IN (SELECT value FROM json_each(?2))",
            )
            .map_err(StoreError::new)?;
        let rows = statement
            .query_map(
                params![tenant, Value::from(scopes.clone()).to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(StoreError::new)?;
        let mut versions = Vec::new();
        for row in rows {
            let (scope, body) = row.map_err(StoreError::new)?;
            let order = scopes.iter().position(|wanted| *wanted == scope);
            versions.push((order, from_json::<PolicyVersion>(&body)?));
        }
        versions.sort_by_key(|(order, _)| *order);
        Ok(versions.into_iter().map(|(_, version)| version).collect())
    }

    fn journal(&self, tenant: &str, scope: &str) -> Result<Vec<StoredEntry>, StoreError> {
        let conn = self.conn()?;
        let mut statement = conn
            .prepare(&format!(
                "SELECT {ENTRY_COLUMNS} FROM entries WHERE tenant = ?1 AND scope = ?2 ORDER BY seq"
            ))
            .map_err(StoreError::new)?;
        let rows = statement
            .query_map(params![tenant, scope], stored_entry)
            .map_err(StoreError::new)?;
        rows.map(|entry| entry.map_err(StoreError::new)).collect()
    }

    fn journals(&self, tenant: &str) -> Result<Vec<StoredJournal>, StoreError> {
        let conn = self.conn()?;
        // One statement reads one snapshot, so no write lands between journals.
        let mut statement = conn
            .prepare(&format!(
                "SELECT {ENTRY_COLUMNS} FROM entries WHERE tenant = ?1 ORDER BY scope, seq"
            ))
            .map_err(StoreError::new)?;
        let rows = statement
            .query_map([tenant], stored_entry)
            .map_err(StoreError::new)?;
        let mut journals: Vec<StoredJournal> = Vec::new();
        for entry in rows {
            let entry = entry.map_err(StoreError::new)?;
            match journals.last_mut() {
                Some(journal) if journal.scope == entry.scope => journal.entries.push(entry),
                _ => journals.push(StoredJournal {
                    scope: entry.scope.clone(),
                    entries: vec![entry],
                }),
            }
        }
        Ok(journals)
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
        tx.execute("DELETE FROM policies WHERE tenant = ?1", [tenant])
            .map_err(StoreError::new)?;
        for projection in projections {
            apply_projection(&tx, tenant, projection)?;
        }
        tx.commit().map_err(StoreError::new)?;
        Ok(true)
    }
}

/// The columns [`stored_entry`] reads, in its order.
const ENTRY_COLUMNS: &str = "scope, id, seq, kind, actor, channel, recorded_at, content,
                             content_hash, prev_hash, entry_hash";

/// One `entries` row, selected as [`ENTRY_COLUMNS`], exactly as stored.
fn stored_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEntry> {
    Ok(StoredEntry {
        scope: row.get(0)?,
        id: row.get(1)?,
        // A negative number can only be tampering; verification reports it as a missing entry.
        seq: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
        kind: row.get(3)?,
        actor: row.get(4)?,
        channel: row.get(5)?,
        recorded_at: row.get(6)?,
        content: row.get(7)?,
        content_hash: row.get(8)?,
        prev_hash: row.get(9)?,
        entry_hash: row.get(10)?,
    })
}

/// The sequence number of the last entry in one scope's journal, or 0 when it has none.
fn journal_end(tx: &Transaction<'_>, tenant: &str, scope: &str) -> Result<u64, StoreError> {
    let seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM entries WHERE tenant = ?1 AND scope = ?2",
            params![tenant, scope],
            |row| row.get(0),
        )
        .map_err(StoreError::new)?;
    u64::try_from(seq).map_err(StoreError::new)
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

/// Switches the database to WAL. SQLite doesn't wait on the busy timeout while it changes
/// journal mode, so when several processes open the same new database at once, the ones that
/// lose retry until the timeout instead of failing.
fn use_wal(conn: &Connection) -> Result<(), StoreError> {
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        match conn.pragma_update(None, "journal_mode", "wal") {
            Ok(()) => return Ok(()),
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == rusqlite::ErrorCode::DatabaseBusy
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(StoreError::new(error)),
        }
    }
}

/// The schema version stamped in the database: 0 when none was ever set.
fn schema_version(conn: &Connection) -> Result<i64, StoreError> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(StoreError::new)
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
        Projection::PutPolicy(version) => put_policy(tx, tenant, &version),
    }
}

/// Makes `version` the policy in effect in its scope.
fn put_policy(
    tx: &Transaction<'_>,
    tenant: &str,
    version: &PolicyVersion,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO policies (tenant, scope, version, body) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (tenant, scope) DO UPDATE SET version = excluded.version, body = excluded.body",
        params![
            tenant,
            version.scope.to_string(),
            version.id,
            to_json(version)?.to_string(),
        ],
    )
    .map(drop)
    .map_err(StoreError::new)
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
