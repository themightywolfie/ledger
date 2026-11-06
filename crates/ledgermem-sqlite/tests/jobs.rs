//! The job queue is durable: a check queued before a restart runs after it, and a job a runner
//! leased but never finished is run again once its lease runs out.

use std::path::Path;
use std::sync::Arc;

use ledgermem_core::{
    ApplyPolicy, Caller, Channel, ClaimInput, Ledger, Observation, Permission, Recall, Remember,
    RememberOutcome, ScriptedVerifier, Status, Store, TestClock, Verification, VerifierName,
    Verifiers,
};
use ledgermem_sqlite::SqliteStore;
use serde_json::json;
use time::Duration;
use time::macros::datetime;

fn coder() -> Caller {
    Caller::agent("local", "coder")
        .with_user("local")
        .with_project("billing-api")
        .with_permissions([Permission::Read, Permission::Propose])
}

/// A ledger on the database at `path` whose test_run checks in the background and finds the
/// tests passing.
fn ledger(path: &Path, clock: Arc<TestClock>) -> Ledger {
    let test_run = ScriptedVerifier::in_background();
    test_run.script(
        "repo:acme/billing-api",
        Observation::pass(json!("passing"), "412 of 412 tests pass"),
    );
    let verifiers = Verifiers::new().with(
        VerifierName::try_from("test_run".to_owned()).unwrap(),
        Arc::new(test_run),
    );
    Ledger::new(Arc::new(SqliteStore::open(path).unwrap()), clock).with_verifiers(verifiers)
}

/// Opens the tenant, applies the software-delivery pack and remembers a claim whose check is
/// queued.
async fn queue_a_check(ledger: &Ledger) -> RememberOutcome {
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
    assert_eq!(written.results[0].verification, Verification::Pending);
    written
}

async fn recalled_status(ledger: &Ledger) -> Status {
    let recalled = ledger
        .recall(&coder(), Recall::new("tests pass"))
        .await
        .unwrap();
    recalled.answer_set[0].status
}

#[tokio::test]
async fn a_check_queued_before_a_restart_runs_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let clock = Arc::new(TestClock::new(datetime!(2026-11-01 09:00 UTC)));
    let before = ledger(&path, clock.clone());
    queue_a_check(&before).await;
    drop(before);

    let after = ledger(&path, clock);
    assert_eq!(recalled_status(&after).await, Status::Proposed);

    assert_eq!(after.run_jobs().await.unwrap().ran, 1);
    assert_eq!(recalled_status(&after).await, Status::Verified);
    assert_eq!(after.run_jobs().await.unwrap().ran, 0);
}

#[tokio::test]
async fn a_job_whose_runner_stopped_runs_again_once_its_lease_runs_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let clock = Arc::new(TestClock::new(datetime!(2026-11-01 09:00 UTC)));
    let ledger = ledger(&path, clock.clone());
    queue_a_check(&ledger).await;

    // Another process leases the job, then stops before it finishes.
    let now = datetime!(2026-11-01 09:00 UTC);
    let other = SqliteStore::open(&path).unwrap();
    let leased = other.lease_job(now, now + Duration::minutes(15)).unwrap();
    assert!(leased.is_some(), "the queued job is there to lease");

    assert_eq!(ledger.run_jobs().await.unwrap().ran, 0);
    assert_eq!(recalled_status(&ledger).await, Status::Proposed);

    clock.advance(Duration::minutes(16));
    assert_eq!(ledger.run_jobs().await.unwrap().ran, 1);
    assert_eq!(recalled_status(&ledger).await, Status::Verified);
}

#[tokio::test]
async fn finishing_a_write_runs_its_check_and_settles_its_result() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(datetime!(2026-11-01 09:00 UTC)));
    let ledger = ledger(&dir.path().join("ledgermem.db"), clock);
    let written = queue_a_check(&ledger).await;
    // The claim is memory from the moment it is written, before any check has run.
    assert_eq!(recalled_status(&ledger).await, Status::Proposed);

    let finished = ledger.finish_remember(&coder(), written).await.unwrap();

    let result = &finished.results[0];
    assert_eq!(result.verification, Verification::Done);
    assert_eq!(result.status, Status::Verified);
    assert!(result.trusted);
    assert_eq!(
        ledger.run_jobs().await.unwrap().ran,
        0,
        "the job is finished"
    );
}

#[tokio::test]
async fn finishing_a_write_leaves_a_check_another_runner_holds_pending() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledgermem.db");
    let clock = Arc::new(TestClock::new(datetime!(2026-11-01 09:00 UTC)));
    let ledger = ledger(&path, clock.clone());
    let written = queue_a_check(&ledger).await;
    let now = datetime!(2026-11-01 09:00 UTC);
    let other = SqliteStore::open(&path).unwrap();
    other.lease_job(now, now + Duration::minutes(15)).unwrap();

    let held = ledger.finish_remember(&coder(), written).await.unwrap();
    assert_eq!(held.results[0].verification, Verification::Pending);
    assert_eq!(held.results[0].status, Status::Proposed);

    // That runner stopped, so once its lease runs out the check is whoever asks next's to run.
    clock.advance(Duration::minutes(16));
    let finished = ledger.finish_remember(&coder(), held).await.unwrap();
    assert_eq!(finished.results[0].verification, Verification::Done);
    assert_eq!(finished.results[0].status, Status::Verified);
}

#[tokio::test]
async fn finishing_a_write_the_job_runner_already_checked_reports_what_it_found() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::new(datetime!(2026-11-01 09:00 UTC)));
    let ledger = ledger(&dir.path().join("ledgermem.db"), clock);
    let written = queue_a_check(&ledger).await;
    assert_eq!(ledger.run_jobs().await.unwrap().ran, 1);

    let finished = ledger.finish_remember(&coder(), written).await.unwrap();

    assert_eq!(finished.results[0].verification, Verification::Done);
    assert_eq!(finished.results[0].status, Status::Verified);
}
