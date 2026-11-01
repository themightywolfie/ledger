//! `ledgermem verify` and `ledgermem reindex`, run as an operator would run them.

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use ledgermem_core::{Caller, ClaimInput, Ledger, Permission, Remember, SystemClock};
use ledgermem_sqlite::SqliteStore;
use serde_json::{Value, json};

/// Writes three deployed versions into the `project:billing-api` journal of the database at `db`.
async fn database_with_three_entries(db: &Path) {
    let ledger = Ledger::new(
        Arc::new(SqliteStore::open(db).unwrap()),
        Arc::new(SystemClock),
    );
    ledger.open_tenant("local").await.unwrap();
    let coder = Caller::agent("local", "coder")
        .with_project("billing-api")
        .with_permissions([Permission::Propose]);
    for version in ["2.13.2", "2.14.0", "2.15.0"] {
        let claim = ClaimInput {
            subject: "service:billing-api".into(),
            predicate: "deployed_version".into(),
            value: json!(version),
            text: format!("billing-api is running {version} in production"),
        };
        let remember = Remember {
            claims: vec![claim],
        };
        ledger.remember(&coder, remember).await.unwrap();
    }
}

fn tamper_with_entry_two(db: &Path) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let changed = conn
        .execute(
            "UPDATE entries SET content = replace(content, '2.14.0', '2.99.0') WHERE seq = 2",
            [],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

fn ledgermem(args: &[&str], db: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ledgermem"))
        .args(args)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::null())
        .output()
        .expect("run ledgermem")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test]
async fn verify_reports_an_intact_journal_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    database_with_three_entries(&db).await;

    let output = ledgermem(&["verify"], &db);

    assert!(output.status.success(), "{output:?}");
    let stdout = stdout(&output);
    assert!(stdout.contains("project:billing-api"), "{stdout}");
    assert!(stdout.contains("ok"), "{stdout}");
    assert!(stdout.contains("3 entries"), "{stdout}");
}

#[tokio::test]
async fn verify_names_the_first_broken_entry_and_exits_three() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    database_with_three_entries(&db).await;
    tamper_with_entry_two(&db);

    let output = ledgermem(&["verify"], &db);

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stdout = stdout(&output);
    assert!(stdout.contains("BROKEN at entry 2"), "{stdout}");
    assert!(stdout.contains("content"), "{stdout}");
}

#[tokio::test]
async fn verify_prints_the_chain_report_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    database_with_three_entries(&db).await;
    tamper_with_entry_two(&db);

    let output = ledgermem(&["verify", "--json"], &db);

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let report: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["ok"], json!(false));
    assert_eq!(report["scopes"][0]["scope"], json!("project:billing-api"));
    assert_eq!(report["scopes"][0]["entries"], json!(3));
    assert_eq!(
        report["scopes"][0]["broken_at"],
        json!({ "seq": 2, "problem": "content_hash_mismatch" })
    );
}

#[tokio::test]
async fn reindex_rebuilds_the_projections_and_reports_what_it_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    database_with_three_entries(&db).await;

    let output = ledgermem(&["reindex", "--json"], &db);

    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report, json!({ "scopes": 1, "entries": 3, "claims": 3 }));

    let output = ledgermem(&["reindex"], &db);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("3 claims"), "{}", stdout(&output));
}

#[tokio::test]
async fn reindex_refuses_a_journal_that_does_not_verify() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    database_with_three_entries(&db).await;
    tamper_with_entry_two(&db);

    let output = ledgermem(&["reindex"], &db);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("project:billing-api is broken at entry 2"),
        "{stderr}"
    );
}
