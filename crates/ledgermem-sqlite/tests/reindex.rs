//! Reindexing rebuilds every projection from the journals (ADR-0010), and reads come back exactly
//! as they were.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::LateWriteStore;
use ledgermem_core::{
    ApplyPolicy, Caller, Channel, ClaimInput, Commit, EntryKind, JournalQuery, Ledger, NewEntry,
    Permission, PolicyQuery, Projection, Recall, ReindexReport, Remember, Scope, ScopeLevel,
    TestClock,
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

fn operator() -> Caller {
    Caller::operator("local", "test", Channel::Cli)
        .with_permissions([Permission::Operator, Permission::Admin])
}

fn claim(predicate: &str, value: &str, text: &str) -> Remember {
    Remember {
        claims: vec![ClaimInput::new(
            "service:billing-api",
            predicate,
            json!(value),
            text,
        )],
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
    let recall = Recall::new("who owns billing-api");
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

#[tokio::test]
async fn reindex_rebuilds_the_policy_in_effect_from_the_versions_journaled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_claims(&path).await;
    // The second tenant version replaces the first, and the project overlays the second.
    let versions = [
        (None, "version: 1\nextends: [software-delivery]\n"),
        (None, "version: 1\nextends: [personal-assistant]\n"),
        (
            Some("billing-api"),
            "version: 1\npredicates:\n  rollout_percent: { value_type: number }\n",
        ),
    ];
    for (project, document) in versions {
        let apply = ApplyPolicy {
            document: document.into(),
            project: project.map(Into::into),
        };
        ledger.apply_policy(&operator(), apply).await.unwrap();
    }
    let query = || PolicyQuery {
        project: Some("billing-api".into()),
    };
    let before = ledger.policy(&coder(), query()).await.unwrap();
    assert_eq!(before.packs, ["personal-assistant"]);

    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DELETE FROM policies;")
        .unwrap();
    assert!(
        ledger
            .policy(&coder(), query())
            .await
            .unwrap()
            .versions
            .is_empty()
    );

    let report = ledger.reindex(&operator()).await.unwrap();

    assert_eq!(report.entries, 6, "three claims and three policy versions");
    assert_eq!(ledger.policy(&coder(), query()).await.unwrap(), before);
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
    let store = LateWriteStore::new(SqliteStore::open(&dir.path().join("ledgermem.db")).unwrap());
    *store.late_write.lock().unwrap() = Some(late_claim());
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
    let recall = Recall::new("billing-api running");
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
