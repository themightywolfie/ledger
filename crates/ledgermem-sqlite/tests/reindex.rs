//! Reindexing rebuilds every projection from the journals (ADR-0010), and reads come back exactly
//! as they were.

use std::path::Path;
use std::sync::{Arc, Mutex};

use ledgermem_core::{
    Caller, Channel, Claim, ClaimInput, Commit, EntryKind, JournalHead, JournalQuery, Ledger,
    NewEntry, Permission, Projection, Recall, ReindexReport, Remember, Scope, ScopeLevel, Store,
    StoreError, StoredEntry, TestClock,
};
use ledgermem_sqlite::SqliteStore;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::macros::datetime;

fn coder() -> Caller {
    Caller::agent("local", "coder")
        .with_user("local")
        .with_project("billing-api")
        .with_permissions([Permission::Read, Permission::Propose])
}

/// Bound to the user scope only, so its claims land in `user:local`.
fn assistant() -> Caller {
    Caller::agent("local", "assistant")
        .with_user("local")
        .with_permissions([Permission::Read, Permission::Propose])
}

fn operator() -> Caller {
    Caller::operator("local", "test", Channel::Cli)
        .with_permissions([Permission::Operator, Permission::Admin])
}

fn claim(predicate: &str, value: &str, text: &str) -> Remember {
    Remember {
        claims: vec![ClaimInput {
            subject: "service:billing-api".into(),
            predicate: predicate.into(),
            value: json!(value),
            text: text.into(),
        }],
    }
}

/// Three claims across two scopes. The last two have identical text, so they tie when recall
/// ranks them, and their order has to survive a rebuild too.
async fn ledger_with_claims(path: &Path) -> Ledger {
    let clock = Arc::new(TestClock::new(datetime!(2026-10-26 09:00 UTC)));
    let ledger = Ledger::new(Arc::new(SqliteStore::open(path).unwrap()), clock);
    ledger.open_tenant("local").await.unwrap();
    let owned = "billing-api is owned by the payments team";
    let writes = [
        (
            coder(),
            claim(
                "deployed_version",
                "2.14.0",
                "billing-api is running 2.14.0",
            ),
        ),
        (assistant(), claim("owner", "team:payments", owned)),
        (coder(), claim("owner", "team:payments", owned)),
    ];
    for (caller, remember) in writes {
        ledger.remember(&caller, remember).await.unwrap();
    }
    ledger
}

/// Everything a reader can see: a recall that matches all three claims, and both journals.
async fn reads(ledger: &Ledger) -> Value {
    let recall = Recall {
        query: "who owns billing-api".into(),
        limit: None,
    };
    let mut reads =
        vec![serde_json::to_value(ledger.recall(&coder(), recall).await.unwrap()).unwrap()];
    for scope in [
        Scope::new(ScopeLevel::Project, "billing-api"),
        Scope::new(ScopeLevel::User, "local"),
    ] {
        let journal = ledger
            .journal(&coder(), JournalQuery { scope })
            .await
            .unwrap();
        reads.push(serde_json::to_value(journal).unwrap());
    }
    Value::Array(reads)
}

#[tokio::test]
async fn reindex_rebuilds_lost_projections_and_reads_come_back_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_claims(&path).await;
    let before = reads(&ledger).await;
    assert_eq!(before[0]["answer_set"].as_array().unwrap().len(), 3);

    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DELETE FROM claims_fts; DELETE FROM claims;")
        .unwrap();
    assert_eq!(reads(&ledger).await[0]["answer_set"], json!([]));

    let report = ledger.reindex(&operator()).await.unwrap();

    assert_eq!(
        report,
        ReindexReport {
            scopes: 2,
            entries: 3,
            claims: 3
        }
    );
    assert_eq!(reads(&ledger).await, before);
}

#[tokio::test]
async fn reindex_refuses_to_rebuild_from_a_journal_that_does_not_verify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_claims(&path).await;
    let before = reads(&ledger).await;

    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE entries SET content = replace(content, '2.14.0', '2.99.0')
             WHERE scope = 'project:billing-api' AND seq = 1",
            [],
        )
        .unwrap();

    let error = ledger.reindex(&operator()).await.unwrap_err();

    assert_eq!(error.code(), "chain_broken");
    assert!(
        error.to_string().contains("project:billing-api"),
        "the error names the broken journal: {error}"
    );
    assert_eq!(
        reads(&ledger).await[0],
        before[0],
        "recall is served from the projections as they were"
    );
}

/// A store that lets one more write land between reindex reading the journals and replacing the
/// projections, the way a `ledgermem mcp` process sharing the database could.
struct InterruptedStore {
    inner: SqliteStore,
    late_write: Mutex<Option<Commit>>,
}

impl Store for InterruptedStore {
    fn tenant_exists(&self, tenant: &str) -> Result<bool, StoreError> {
        self.inner.tenant_exists(tenant)
    }
    fn create_tenant(&self, tenant: &str, at: OffsetDateTime) -> Result<(), StoreError> {
        self.inner.create_tenant(tenant, at)
    }
    fn commit(&self, tenant: &str, commit: Commit) -> Result<(), StoreError> {
        self.inner.commit(tenant, commit)
    }
    fn search(
        &self,
        tenant: &str,
        scopes: &[Scope],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Claim>, StoreError> {
        self.inner.search(tenant, scopes, query, limit)
    }
    fn journal(&self, tenant: &str, scope: &str) -> Result<Vec<StoredEntry>, StoreError> {
        self.inner.journal(tenant, scope)
    }
    fn scopes(&self, tenant: &str) -> Result<Vec<String>, StoreError> {
        self.inner.scopes(tenant)
    }
    fn replace_projections(
        &self,
        tenant: &str,
        heads: &[JournalHead],
        projections: Vec<Projection>,
    ) -> Result<bool, StoreError> {
        if let Some(late_write) = self.late_write.lock().unwrap().take() {
            self.inner.commit(tenant, late_write)?;
        }
        self.inner.replace_projections(tenant, heads, projections)
    }
}

/// A claim written straight through the store port, as another process's `remember` would.
fn late_claim() -> Commit {
    let content = json!({
        "id": "clm_01LATECLAIM000000000000000",
        "scope": "project:billing-api",
        "subject": "service:billing-api",
        "predicate": "deployed_version",
        "value": "2.15.0",
        "text": "billing-api is running 2.15.0",
        "source": { "kind": "agent_claim", "actor": "agent:other", "channel": "agent" },
        "status": "proposed",
        "valid_from": "2026-10-26T09:00:00Z",
        "valid_to": null,
        "recorded_at": "2026-10-26T09:00:00Z",
    });
    Commit {
        projections: vec![Projection::PutClaim(
            serde_json::from_value(content.clone()).unwrap(),
        )],
        entries: vec![NewEntry {
            id: "ent_01LATEENTRY000000000000000".into(),
            scope: Scope::new(ScopeLevel::Project, "billing-api"),
            kind: EntryKind::ClaimWritten,
            actor: "agent:other".into(),
            channel: Channel::Agent,
            recorded_at: datetime!(2026-10-26 09:00 UTC),
            content,
        }],
    }
}

#[tokio::test]
async fn a_claim_written_while_reindex_runs_survives_the_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let store = InterruptedStore {
        inner: SqliteStore::open(&dir.path().join("ledgermem.db")).unwrap(),
        late_write: Mutex::new(Some(late_claim())),
    };
    let clock = Arc::new(TestClock::new(datetime!(2026-10-26 09:00 UTC)));
    let ledger = Ledger::new(Arc::new(store), clock);
    ledger.open_tenant("local").await.unwrap();
    ledger
        .remember(
            &coder(),
            claim(
                "deployed_version",
                "2.14.0",
                "billing-api is running 2.14.0",
            ),
        )
        .await
        .unwrap();

    let report = ledger.reindex(&operator()).await.unwrap();

    assert_eq!(
        report.entries, 2,
        "the rebuild includes the late entry: {report:?}"
    );
    let recall = Recall {
        query: "billing-api running".into(),
        limit: None,
    };
    let recalled = ledger.recall(&coder(), recall).await.unwrap();
    let values: Vec<_> = recalled
        .answer_set
        .iter()
        .map(|claim| claim.value.clone())
        .collect();
    assert!(
        values.contains(&json!("2.15.0")),
        "the late claim is still recalled: {values:?}"
    );
}
