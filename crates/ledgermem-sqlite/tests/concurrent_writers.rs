//! Two stores on one database file, as two `ledgermem mcp` processes on one machine would be.

use ledgermem_core::{Channel, Commit, EntryKind, NewEntry, Scope, ScopeLevel, Store};
use ledgermem_sqlite::SqliteStore;
use serde_json::json;
use time::macros::datetime;

const COMMITS_PER_STORE: u64 = 50;

fn one_entry(id: String, scope: &Scope) -> Commit {
    Commit {
        entries: vec![NewEntry {
            id,
            scope: scope.clone(),
            kind: EntryKind::ClaimWritten,
            actor: "agent:test".into(),
            channel: Channel::Agent,
            recorded_at: datetime!(2026-10-25 09:00 UTC),
            content: json!({}),
        }],
        projections: vec![],
    }
}

#[test]
fn concurrent_writers_number_a_scopes_entries_without_gaps_or_failures() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let stores = [
        SqliteStore::open(&path).unwrap(),
        SqliteStore::open(&path).unwrap(),
    ];
    stores[0]
        .create_tenant("local", datetime!(2026-10-25 09:00 UTC))
        .unwrap();
    let scope = Scope::new(ScopeLevel::Project, "billing-api");

    std::thread::scope(|threads| {
        for (n, store) in stores.iter().enumerate() {
            let scope = &scope;
            threads.spawn(move || {
                for i in 0..COMMITS_PER_STORE {
                    store
                        .commit("local", one_entry(format!("ent_{n}_{i}"), scope))
                        .expect("commit");
                }
            });
        }
    });

    let seqs: Vec<u64> = stores[0]
        .journal("local", &scope)
        .unwrap()
        .iter()
        .map(|entry| entry.seq)
        .collect();
    assert_eq!(seqs, (1..=2 * COMMITS_PER_STORE).collect::<Vec<_>>());
}
