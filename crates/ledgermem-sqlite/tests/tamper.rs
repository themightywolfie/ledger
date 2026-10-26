//! Editing a stored journal behind Ledger's back is caught by verification, at the first entry
//! that no longer holds.

use std::path::Path;
use std::sync::Arc;

use ledgermem_core::{
    BrokenLink, Caller, ChainProblem, Channel, ClaimInput, EntryKind, Ledger, NewEntry, Permission,
    Remember, Scope, ScopeLevel, TestClock,
};
use ledgermem_sqlite::SqliteStore;
use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;

/// A ledger on a database file whose `project:billing-api` journal holds three entries, one per
/// deployed version: 2.13.2, 2.14.0 and 2.15.0.
async fn ledger_with_three_entries(path: &Path) -> Ledger {
    let clock = Arc::new(TestClock::new(datetime!(2026-10-26 09:00 UTC)));
    let ledger = Ledger::new(Arc::new(SqliteStore::open(path).unwrap()), clock);
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
    ledger
}

fn operator() -> Caller {
    Caller::operator("local", "test", Channel::Cli).with_permissions([Permission::Operator])
}

/// Edits the database directly, the way someone tampering with the file would.
fn tamper(path: &Path, sql: &str) {
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.execute(sql, []).unwrap(),
        1,
        "tampering changed no entry"
    );
}

async fn first_broken_link(ledger: &Ledger) -> Option<BrokenLink> {
    let report = ledger.verify_journals(&operator()).await.unwrap();
    assert_eq!(report.ok, report.scopes[0].broken_at.is_none());
    report.scopes[0].broken_at.clone()
}

#[tokio::test]
async fn an_entry_whose_content_was_changed_breaks_the_chain_there() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_three_entries(&path).await;
    assert_eq!(first_broken_link(&ledger).await, None);

    tamper(
        &path,
        "UPDATE entries SET content = replace(content, '2.14.0', '2.99.0') WHERE seq = 2",
    );

    assert_eq!(
        first_broken_link(&ledger).await,
        Some(BrokenLink {
            seq: 2,
            problem: ChainProblem::ContentHashMismatch
        })
    );
}

#[tokio::test]
async fn an_entry_whose_actor_was_changed_breaks_the_chain_there() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_three_entries(&path).await;

    tamper(
        &path,
        "UPDATE entries SET actor = 'operator:someone-else' WHERE seq = 2",
    );

    assert_eq!(
        first_broken_link(&ledger).await,
        Some(BrokenLink {
            seq: 2,
            problem: ChainProblem::EntryHashMismatch
        })
    );
}

#[tokio::test]
async fn a_deleted_entry_breaks_the_chain_where_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_three_entries(&path).await;

    tamper(&path, "DELETE FROM entries WHERE seq = 2");

    assert_eq!(
        first_broken_link(&ledger).await,
        Some(BrokenLink {
            seq: 2,
            problem: ChainProblem::MissingEntry
        })
    );
}

#[tokio::test]
async fn an_entry_forged_with_fresh_hashes_breaks_the_chain_at_the_next_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_three_entries(&path).await;

    // Rewrite entry 2 so that it is consistent with itself: new content, a content hash and an
    // entry hash recomputed for that content, still linked to entry 1.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let (id, actor, recorded_at, content, prev_hash): (String, String, String, String, String) =
        conn.query_row(
            "SELECT id, actor, recorded_at, content, prev_hash FROM entries WHERE seq = 2",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    let forged = NewEntry {
        id,
        scope: Scope::new(ScopeLevel::Project, "billing-api"),
        kind: EntryKind::ClaimWritten,
        actor,
        channel: Channel::Agent,
        recorded_at: OffsetDateTime::parse(&recorded_at, &Rfc3339).unwrap(),
        content: serde_json::from_str(&content.replace("2.14.0", "2.99.0")).unwrap(),
    }
    .seal(2, prev_hash);
    let updated = conn
        .execute(
            "UPDATE entries SET content = ?1, content_hash = ?2, entry_hash = ?3 WHERE seq = 2",
            rusqlite::params![forged.content, forged.content_hash, forged.entry_hash],
        )
        .unwrap();
    assert_eq!(updated, 1);

    assert_eq!(
        first_broken_link(&ledger).await,
        Some(BrokenLink {
            seq: 3,
            problem: ChainProblem::PrevHashMismatch
        })
    );
}

#[tokio::test]
async fn an_entry_whose_kind_was_changed_to_one_this_build_does_not_know_breaks_the_chain_there() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_three_entries(&path).await;

    tamper(
        &path,
        "UPDATE entries SET kind = 'claim_retracted' WHERE seq = 2",
    );

    assert_eq!(
        first_broken_link(&ledger).await,
        Some(BrokenLink {
            seq: 2,
            problem: ChainProblem::EntryHashMismatch
        })
    );
}

#[tokio::test]
async fn an_entry_whose_content_was_replaced_with_something_unreadable_breaks_the_chain_there() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let ledger = ledger_with_three_entries(&path).await;

    tamper(
        &path,
        "UPDATE entries SET content = 'not json' WHERE seq = 2",
    );

    assert_eq!(
        first_broken_link(&ledger).await,
        Some(BrokenLink {
            seq: 2,
            problem: ChainProblem::ContentHashMismatch
        })
    );
}
