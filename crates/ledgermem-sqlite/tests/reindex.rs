//! Reindexing rebuilds every projection from the journals (ADR-0010), and reads come back exactly
//! as they were.

use std::path::Path;
use std::sync::Arc;

use ledgermem_core::{
    Caller, Channel, ClaimInput, JournalQuery, Ledger, Permission, Recall, ReindexReport, Remember,
    Scope, ScopeLevel, TestClock,
};
use ledgermem_sqlite::SqliteStore;
use serde_json::{Value, json};
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

fn admin() -> Caller {
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

    let report = ledger.reindex(&admin()).await.unwrap();

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

    let error = ledger.reindex(&admin()).await.unwrap_err();

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
