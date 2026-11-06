//! An attestation decides from the claim as it stands when the attestation commits, even when
//! another process writes to the claim's scope after it read the claim.

mod common;

use std::sync::Arc;

use common::LateWriteStore;
use ledgermem_core::{
    ApplyPolicy, Attest, Caller, Channel, Claim, ClaimInput, Commit, EntryKind, JournalQuery,
    Ledger, NewEntry, Observation, Permission, Projection, Recall, Remember, RetractionReason,
    Scope, ScopeLevel, ScriptedVerifier, Status, Store, TestClock, VerifierName, Verifiers,
};
use ledgermem_sqlite::SqliteStore;
use serde_json::json;
use time::macros::datetime;

fn coder() -> Caller {
    Caller::agent("local", "coder")
        .with_user("local")
        .with_project("billing-api")
        .with_permissions([Permission::Read, Permission::Propose, Permission::Attest])
}

/// What another process's attestation writes when its test run finds the tests failing.
fn refutation(claim: &Claim) -> Commit {
    let mut refuted = claim.clone();
    refuted.status = Status::Retracted;
    refuted.reason = Some(RetractionReason::Refuted);
    Commit {
        entries: vec![NewEntry {
            id: "ent_01RACINGENTRY0000000000000".into(),
            scope: claim.scope.clone(),
            kind: EntryKind::ClaimRetracted,
            actor: "agent:other".into(),
            channel: Channel::Agent,
            recorded_at: datetime!(2026-11-01 09:00 UTC),
            content: json!({ "claim_id": claim.id, "reason": "refuted" }),
        }],
        projections: vec![Projection::PutClaim(refuted)],
        ..Commit::default()
    }
}

#[tokio::test]
async fn an_attestation_that_races_a_refutation_does_not_bring_the_claim_back() {
    let store = Arc::new(LateWriteStore::new(SqliteStore::open_in_memory().unwrap()));
    let test_run = Arc::new(ScriptedVerifier::default());
    let verifiers = Verifiers::new().with(
        VerifierName::try_from("test_run".to_owned()).unwrap(),
        test_run.clone(),
    );
    let clock = Arc::new(TestClock::new(datetime!(2026-11-01 09:00 UTC)));
    let ledger = Ledger::new(store.clone(), clock).with_verifiers(verifiers);
    ledger.open_tenant("local").await.unwrap();
    let operator =
        Caller::operator("local", "test", Channel::Cli).with_permissions([Permission::Operator]);
    let policy = ApplyPolicy {
        document: "version: 1\nextends: [software-delivery]\n".into(),
        project: None,
    };
    ledger.apply_policy(&operator, policy).await.unwrap();
    let remember = Remember {
        claims: vec![ClaimInput::new(
            "repo:acme/billing-api",
            "test_status",
            json!("passing"),
            "all tests pass",
        )],
    };
    let written = ledger.remember(&coder(), remember).await.unwrap();
    let claim_id = written.results[0].claim_id.clone();
    let (claim, _) = store.claim("local", &claim_id).unwrap().expect("the claim");

    // This process's test run passes, but another's refutation lands first.
    test_run.script(
        "repo:acme/billing-api",
        Observation::pass(json!("passing"), "412 of 412 tests pass"),
    );
    *store.late_write.lock().unwrap() = Some(refutation(&claim));
    let attest = Attest {
        claim_id,
        evidence: Vec::new(),
        verify_now: true,
    };
    let error = ledger.attest(&coder(), attest).await.unwrap_err();

    assert_eq!(error.code(), "invalid_claim", "{error}");
    let journal = ledger
        .journal(
            &coder(),
            JournalQuery {
                scope: Scope::new(ScopeLevel::Project, "billing-api"),
            },
        )
        .await
        .unwrap();
    let kinds: Vec<_> = journal.iter().map(|entry| entry.kind).collect();
    assert_eq!(kinds, [EntryKind::ClaimWritten, EntryKind::ClaimRetracted]);
    let recalled = ledger
        .recall(&coder(), Recall::new("tests pass"))
        .await
        .unwrap();
    assert!(recalled.answer_set.is_empty(), "{recalled:?}");
}
