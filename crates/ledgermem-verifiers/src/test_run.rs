//! The `test_run` verifier: checks whether a repository's tests pass, against its JUnit report.
//!
//! A claim on `test_status` says a repository's tests are `passing` or `failing`. The verifier
//! reads the JUnit report configured for the claim's subject, counts the test cases that failed
//! or errored, and passes the claim when the count agrees with it.
//!
//! With the `rerun` parameter set, the verifier first runs the repository's configured test
//! command, which must write a fresh report: a run that dies before it does is inconclusive,
//! never answered from the old report. That takes as long as the tests do, so such a check
//! always runs in the background.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use ledgermem_core::{Check, Observation, Verifier, Verifying};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Map, Value, json};
use tokio::process::Command;

use crate::blocking;

/// The largest report the verifier reads.
const MAX_REPORT: u64 = 64 * 1024 * 1024;

/// The longest a re-run may take, however long is configured: shorter than the lease a job holds,
/// so a check still running is never taken over by another runner.
const MAX_RERUN: Duration = Duration::from_secs(10 * 60);

/// How far a report's write time may read before the re-run that wrote it, since some
/// filesystems keep times only to the second or two.
const WRITE_TIME_SLACK: Duration = Duration::from_secs(2);

/// Checks claims about the repositories it knows the test reports of.
#[derive(Debug, Clone, Default)]
pub struct TestRunVerifier {
    /// Each repository subject, as Ledger keeps it, and where its test results come from.
    repos: BTreeMap<String, TestRunRepo>,
}

/// Where one repository's test results come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRunRepo {
    /// Its JUnit XML report.
    pub junit: PathBuf,
    /// The command that runs its tests again and writes the report, when re-running is set up.
    pub rerun: Option<Rerun>,
}

/// A command that runs a repository's tests again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rerun {
    /// The program and its arguments. No shell runs it.
    pub command: Vec<String>,
    pub cwd: PathBuf,
    /// How long the tests may take before the check gives up, inconclusive. At most ten minutes.
    pub timeout: Duration,
}

/// What a JUnit report says.
struct Report {
    tests: u64,
    /// The test cases with a failure or an error.
    failed: u64,
}

impl TestRunVerifier {
    /// A verifier for `repos`: each a subject such as `repo:acme/billing-api`, and where its test
    /// results come from.
    pub fn new(repos: impl IntoIterator<Item = (String, TestRunRepo)>) -> Self {
        TestRunVerifier {
            repos: repos
                .into_iter()
                .map(|(subject, repo)| (subject.trim().to_lowercase(), repo))
                .collect(),
        }
    }

    /// What the repository's tests say about the claim, or why they couldn't be asked.
    async fn observe(&self, check: Check<'_>) -> Result<Observation, String> {
        let subject = &check.claim.subject;
        let repo = self
            .repos
            .get(subject)
            .ok_or_else(|| format!("no JUnit report is configured for {subject}"))?;
        let claimed = match &check.claim.value {
            Value::String(claimed) if claimed == "passing" || claimed == "failing" => claimed,
            other => {
                return Err(format!(
                    "test_run checks that tests are passing or failing, not {other}"
                ));
            }
        };
        let mut written_since = None;
        if reruns(check.params) {
            let rerun = repo
                .rerun
                .as_ref()
                .ok_or_else(|| format!("re-running {subject}'s tests isn't configured"))?;
            written_since = Some(SystemTime::now());
            run(rerun).await?;
        }
        let junit = repo.junit.clone();
        let report = blocking("reading the report", move || read(&junit, written_since)).await??;
        let (observed, summary) = match report.failed {
            0 => ("passing", format!("{0} of {0} tests pass", report.tests)),
            failed => (
                "failing",
                format!("{failed} of {} tests fail", report.tests),
            ),
        };
        Ok(if observed == claimed {
            Observation::pass(json!(observed), summary)
        } else {
            Observation::fail(json!(observed), summary)
        })
    }
}

impl Verifier for TestRunVerifier {
    fn verify<'a>(&'a self, check: Check<'a>) -> Verifying<'a> {
        Box::pin(async move {
            self.observe(check)
                .await
                .unwrap_or_else(Observation::inconclusive)
        })
    }

    fn runs_in_background(&self, params: &Map<String, Value>) -> bool {
        reruns(params)
    }
}

/// Whether the check runs the tests again before reading the report.
fn reruns(params: &Map<String, Value>) -> bool {
    params.get("rerun") == Some(&Value::Bool(true))
}

/// Runs the tests again. Failing tests are no reason to stop: the report says they failed.
async fn run(rerun: &Rerun) -> Result<(), String> {
    let (program, args) = rerun
        .command
        .split_first()
        .ok_or("the configured test command is empty")?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&rerun.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let timeout = rerun.timeout.min(MAX_RERUN);
    match tokio::time::timeout(timeout, command.status()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(format!("the tests couldn't run: {error}")),
        Err(_) => Err(format!("the tests took longer than {}s", timeout.as_secs())),
    }
}

/// Counts the test cases in the JUnit report at `path`. After a re-run, the report must have been
/// written since it began.
fn read(path: &Path, written_since: Option<SystemTime>) -> Result<Report, String> {
    let shown = path.display();
    let metadata = path
        .metadata()
        .map_err(|error| format!("can't read the JUnit report {shown}: {error}"))?;
    if let Some(began) = written_since {
        let written = metadata
            .modified()
            .map_err(|error| format!("can't tell when {shown} was written: {error}"))?;
        if written + WRITE_TIME_SLACK < began {
            return Err(format!("the tests ran, but didn't write a fresh {shown}"));
        }
    }
    if metadata.len() > MAX_REPORT {
        return Err(format!(
            "{shown} is larger than the {} MiB the verifier reads",
            MAX_REPORT / (1024 * 1024)
        ));
    }
    let xml = std::fs::read_to_string(path)
        .map_err(|error| format!("can't read the JUnit report {shown}: {error}"))?;
    let report = count(&xml).map_err(|why| format!("{shown} isn't a JUnit report: {why}"))?;
    if report.tests == 0 {
        return Err(format!("{shown} has no test cases"));
    }
    Ok(report)
}

/// The test case a JUnit report is in the middle of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    None,
    Passing,
    Failed,
}

/// The test cases in a JUnit report, and how many of them failed or errored, each counting once.
/// A report that ends before its elements close, such as one a test run is still writing, is no
/// report.
fn count(xml: &str) -> Result<Report, String> {
    let mut reader = Reader::from_str(xml);
    let mut report = Report {
        tests: 0,
        failed: 0,
    };
    let mut open = 0_u32;
    let mut case = Case::None;
    loop {
        let event = reader.read_event().map_err(|error| error.to_string())?;
        match &event {
            Event::Start(tag) if tag.local_name().as_ref() == "testcase" => {
                report.tests += 1;
                case = Case::Passing;
            }
            Event::Empty(tag) if tag.local_name().as_ref() == "testcase" => report.tests += 1,
            Event::Start(tag) | Event::Empty(tag)
                if case == Case::Passing
                    && matches!(tag.local_name().as_ref(), "failure" | "error") =>
            {
                report.failed += 1;
                case = Case::Failed;
            }
            Event::End(tag) if tag.local_name().as_ref() == "testcase" => case = Case::None,
            _ => {}
        }
        match event {
            Event::Start(_) => open += 1,
            Event::End(_) => open = open.saturating_sub(1),
            Event::Eof if open > 0 => return Err("it ends before its elements close".into()),
            Event::Eof => return Ok(report),
            _ => {}
        }
    }
}
