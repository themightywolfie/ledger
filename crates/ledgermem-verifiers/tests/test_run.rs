//! The test_run verifier against JUnit report fixtures.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{assert_observed, claim, verify};
use ledgermem_core::Outcome::{Fail, Inconclusive, Pass};
use ledgermem_core::{Claim, Verifier};
use ledgermem_verifiers::{Rerun, TestRunRepo, TestRunVerifier};
use serde_json::{Map, json};

const REPO: &str = "repo:acme/billing-api";

/// Three test cases in two suites, all passing.
const PASSING: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
  <testsuite name="invoices" tests="2" failures="0" errors="0">
    <testcase name="issues an invoice" classname="invoices"/>
    <testcase name="voids an invoice" classname="invoices"><system-out>ok</system-out></testcase>
  </testsuite>
  <testsuite name="refunds" tests="1">
    <testcase name="refunds a payment"/>
  </testsuite>
</testsuites>"#;

/// Four test cases: one fails, one errors, and one fails twice, which still counts once.
const FAILING: &str = r#"<testsuite name="billing" tests="4">
  <testcase name="issues an invoice"/>
  <testcase name="voids an invoice"><failure message="expected void">assertion failed</failure></testcase>
  <testcase name="refunds a payment"><error message="timeout"/></testcase>
  <testcase name="retries"><failure/><failure/></testcase>
</testsuite>"#;

/// A test_run claim about billing-api.
fn tests_are(value: &str) -> Claim {
    claim(REPO, "test_status", json!(value))
}

/// A verifier that reads `report` as billing-api's JUnit report, written into `dir`.
fn verifier_reading(dir: &Path, report: &str) -> (TestRunVerifier, PathBuf) {
    let junit = dir.join("junit.xml");
    std::fs::write(&junit, report).unwrap();
    let repo = TestRunRepo {
        junit: junit.clone(),
        rerun: None,
    };
    (TestRunVerifier::new([(REPO.to_owned(), repo)]), junit)
}

#[tokio::test]
async fn a_report_with_no_failures_or_errors_passes_a_claim_that_the_tests_pass() {
    let dir = tempfile::tempdir().unwrap();
    let (verifier, _) = verifier_reading(dir.path(), PASSING);

    let observed = verify(&verifier, &tests_are("passing"), json!({})).await;
    assert_observed(&observed, Pass, "3 of 3 tests pass");
    assert_eq!(observed.observed, json!("passing"));

    let observed = verify(&verifier, &tests_are("failing"), json!({})).await;
    assert_observed(&observed, Fail, "3 of 3 tests pass");
}

#[tokio::test]
async fn failures_and_errors_count_once_per_test_case_and_refute_a_claim_that_the_tests_pass() {
    let dir = tempfile::tempdir().unwrap();
    let (verifier, _) = verifier_reading(dir.path(), FAILING);

    let observed = verify(&verifier, &tests_are("passing"), json!({})).await;
    assert_observed(&observed, Fail, "3 of 4 tests fail");
    assert_eq!(observed.observed, json!("failing"));

    assert_observed(
        &verify(&verifier, &tests_are("failing"), json!({})).await,
        Pass,
        "3 of 4 tests fail",
    );
}

#[tokio::test]
async fn a_report_written_before_the_claim_still_speaks_for_it() {
    // Run the tests, then say they pass: the report is older than the claim, as it should be.
    let dir = tempfile::tempdir().unwrap();
    let (verifier, junit) = verifier_reading(dir.path(), PASSING);
    let long_ago = SystemTime::UNIX_EPOCH + Duration::from_secs(1_577_836_800);
    std::fs::File::options()
        .write(true)
        .open(&junit)
        .unwrap()
        .set_modified(long_ago)
        .unwrap();

    assert_observed(
        &verify(&verifier, &tests_are("passing"), json!({})).await,
        Pass,
        "3 of 3 tests pass",
    );
}

#[tokio::test]
async fn what_keeps_the_verifier_from_reading_a_report_is_inconclusive() {
    let dir = tempfile::tempdir().unwrap();
    let (verifier, junit) = verifier_reading(dir.path(), "<testsuite><testcase>");

    assert_observed(
        &verify(&verifier, &tests_are("passing"), json!({})).await,
        Inconclusive,
        "isn't a JUnit report",
    );

    std::fs::write(&junit, "<testsuite/>").unwrap();
    assert_observed(
        &verify(&verifier, &tests_are("passing"), json!({})).await,
        Inconclusive,
        "has no test cases",
    );

    std::fs::remove_file(&junit).unwrap();
    assert_observed(
        &verify(&verifier, &tests_are("passing"), json!({})).await,
        Inconclusive,
        "can't read the JUnit report",
    );

    let mut payments = tests_are("passing");
    payments.subject = "repo:acme/payments".into();
    assert_observed(
        &verify(&verifier, &payments, json!({})).await,
        Inconclusive,
        "no JUnit report is configured for repo:acme/payments",
    );

    assert_observed(
        &verify(&verifier, &tests_are("green"), json!({})).await,
        Inconclusive,
        "passing or failing",
    );
}

#[tokio::test]
async fn re_running_the_tests_is_opt_in_and_always_in_the_background() {
    let dir = tempfile::tempdir().unwrap();
    let (reading, _) = verifier_reading(dir.path(), PASSING);
    let rerun = |value: serde_json::Value| -> Map<String, serde_json::Value> {
        serde_json::from_value(json!({ "rerun": value })).unwrap()
    };

    assert!(reading.runs_in_background(&rerun(json!(true))));
    assert!(!reading.runs_in_background(&rerun(json!(false))));
    assert!(!reading.runs_in_background(&Map::new()));

    assert_observed(
        &verify(&reading, &tests_are("passing"), json!({ "rerun": true })).await,
        Inconclusive,
        "re-running repo:acme/billing-api's tests isn't configured",
    );
}

/// A verifier that reads `junit`, and re-runs billing-api's tests with `command` in `dir`.
fn rerunning(dir: &Path, junit: PathBuf, command: &[&str]) -> TestRunVerifier {
    let repo = TestRunRepo {
        junit,
        rerun: Some(Rerun {
            command: command.iter().map(|part| (*part).to_owned()).collect(),
            cwd: dir.to_path_buf(),
            timeout: Duration::from_secs(30),
        }),
    };
    TestRunVerifier::new([(REPO.to_owned(), repo)])
}

#[tokio::test]
async fn a_re_run_that_writes_no_report_is_never_answered_from_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let (_, junit) = verifier_reading(dir.path(), PASSING);
    let long_ago = SystemTime::UNIX_EPOCH + Duration::from_secs(1_577_836_800);
    std::fs::File::options()
        .write(true)
        .open(&junit)
        .unwrap()
        .set_modified(long_ago)
        .unwrap();
    // The "test suite" dies at once, as a build error would.
    let command = if cfg!(windows) {
        ["cmd", "/C", "exit", "1"].as_slice()
    } else {
        ["false"].as_slice()
    };
    let verifier = rerunning(dir.path(), junit, command);

    assert_observed(
        &verify(&verifier, &tests_are("passing"), json!({ "rerun": true })).await,
        Inconclusive,
        "didn't write a fresh",
    );
}

#[tokio::test]
async fn re_running_the_tests_reads_the_report_they_write() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fresh.xml"), PASSING).unwrap();
    let junit = dir.path().join("junit.xml");
    std::fs::write(&junit, FAILING).unwrap();
    // The "test suite" writes a passing report over the failing one.
    let command = if cfg!(windows) {
        ["cmd", "/C", "type", "fresh.xml", ">", "junit.xml"].as_slice()
    } else {
        ["cp", "fresh.xml", "junit.xml"].as_slice()
    };
    let verifier = rerunning(dir.path(), junit, command);

    assert_observed(
        &verify(&verifier, &tests_are("passing"), json!({})).await,
        Fail,
        "3 of 4 tests fail",
    );
    assert_observed(
        &verify(&verifier, &tests_are("passing"), json!({ "rerun": true })).await,
        Pass,
        "3 of 3 tests pass",
    );
}
