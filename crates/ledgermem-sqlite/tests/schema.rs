//! Opening a database whose schema this build doesn't use fails up front, with a reason, instead
//! of failing later on the first write.

use std::sync::Arc;

use ledgermem_core::{ApplyPolicy, Caller, Channel, Ledger, Permission, PolicyQuery, SystemClock};
use ledgermem_sqlite::SqliteStore;

#[test]
fn a_database_from_before_hash_chains_is_refused_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    // The journal table as the first pre-release build created it: no content, previous or entry
    // hashes, and no schema version.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE entries (
                tenant TEXT NOT NULL, scope TEXT NOT NULL, seq INTEGER NOT NULL,
                id TEXT NOT NULL UNIQUE, kind TEXT NOT NULL, actor TEXT NOT NULL,
                channel TEXT NOT NULL, recorded_at TEXT NOT NULL, content TEXT,
                PRIMARY KEY (tenant, scope, seq)
            ) STRICT;",
        )
        .unwrap();

    let error = SqliteStore::open(&path)
        .err()
        .expect("the old database is refused");

    assert!(error.to_string().contains("pre-release"), "{error}");
}

#[test]
fn a_database_from_a_newer_ledgermem_is_refused_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();

    let error = SqliteStore::open(&path)
        .err()
        .expect("the newer database is refused");

    assert!(error.to_string().contains("newer"), "{error}");
}

#[test]
fn a_database_reopens_after_it_was_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    drop(SqliteStore::open(&path).unwrap());

    SqliteStore::open(&path).expect("a database this build created reopens");
}

#[tokio::test]
async fn a_database_from_schema_version_1_is_brought_up_to_date_and_takes_a_policy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    // Version 1 had every table this build creates except policies.
    drop(SqliteStore::open(&path).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE policies; PRAGMA user_version = 1;")
        .unwrap();

    let ledger = Ledger::new(
        Arc::new(SqliteStore::open(&path).unwrap()),
        Arc::new(SystemClock),
    );
    ledger.open_tenant("local").await.unwrap();
    let operator = Caller::operator("local", "test", Channel::Cli)
        .with_permissions([Permission::Read, Permission::Operator]);
    let apply = ApplyPolicy {
        document: "version: 1\nextends: [software-delivery]\n".into(),
        project: None,
    };
    ledger.apply_policy(&operator, apply).await.unwrap();

    let policy = ledger
        .policy(&operator, PolicyQuery { project: None })
        .await
        .unwrap();
    assert_eq!(policy.packs, ["software-delivery"]);
}

#[test]
fn processes_opening_a_new_database_at_the_same_moment_all_succeed() {
    const OPENERS: usize = 8;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let start = std::sync::Barrier::new(OPENERS);

    let failures: Vec<String> = std::thread::scope(|threads| {
        let handles: Vec<_> = (0..OPENERS)
            .map(|_| {
                threads.spawn(|| {
                    start.wait();
                    SqliteStore::open(&path)
                        .err()
                        .map(|error| error.to_string())
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect()
    });

    assert!(failures.is_empty(), "opens failed: {failures:?}");
}
