//! The git verifier against a temporary repository.

mod common;

use std::path::Path;
use std::process::Command;

use common::{assert_observed, claim, verify};
use ledgermem_core::Outcome::{Fail, Inconclusive, Pass};
use ledgermem_verifiers::GitVerifier;
use serde_json::{Value, json};

const REPO: &str = "repo:acme/billing-api";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A clone of billing-api whose `main` has merged `feature/invoices`, with `feature/refunds` not
/// yet merged. Returns the directory, the verifier, and the id of the merged commit.
fn repository() -> (tempfile::TempDir, GitVerifier, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "--quiet", "--initial-branch=main"]);
    git(repo, &["config", "user.name", "Test"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("README.md"), "billing-api\n").unwrap();
    std::fs::create_dir(repo.join("docs")).unwrap();
    std::fs::write(
        repo.join("docs").join("runbook.md"),
        "restart billing-api\n",
    )
    .unwrap();
    git(repo, &["add", "README.md", "docs"]);
    git(repo, &["commit", "--quiet", "--message", "start"]);

    git(repo, &["switch", "--quiet", "--create", "feature/invoices"]);
    std::fs::write(
        repo.join("README.md"),
        "billing-api serves the v2 invoices API\n",
    )
    .unwrap();
    git(
        repo,
        &["commit", "--quiet", "--all", "--message", "invoices"],
    );
    let merged = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["switch", "--quiet", "main"]);
    git(
        repo,
        &[
            "merge",
            "--quiet",
            "--no-ff",
            "--message",
            "merge invoices",
            "feature/invoices",
        ],
    );

    git(repo, &["switch", "--quiet", "--create", "feature/refunds"]);
    std::fs::write(repo.join("REFUNDS.md"), "refunds\n").unwrap();
    git(repo, &["add", "REFUNDS.md"]);
    git(repo, &["commit", "--quiet", "--message", "refunds"]);
    git(repo, &["switch", "--quiet", "main"]);

    let verifier = GitVerifier::new([(REPO.to_owned(), repo.to_path_buf())]);
    (dir, verifier, merged)
}

async fn check(verifier: &GitVerifier, value: Value, params: Value) -> ledgermem_core::Observation {
    verify(verifier, &claim(REPO, "under_test", value), params).await
}

#[tokio::test]
async fn commit_exists_passes_for_a_commit_in_the_repository_and_fails_for_one_that_is_not() {
    let (_dir, verifier, merged) = repository();
    let commit_exists = json!({ "check": "commit_exists" });

    let observed = check(&verifier, json!(&merged[..9]), commit_exists.clone()).await;
    assert_observed(&observed, Pass, &format!("is commit {merged}"));
    assert_eq!(observed.observed, json!(merged));

    let unknown = "0123456789abcdef0123456789abcdef01234567";
    let observed = check(&verifier, json!(unknown), commit_exists).await;
    assert_observed(&observed, Fail, &format!("{REPO} has no commit {unknown}"));
}

#[tokio::test]
async fn branch_exists_passes_for_a_branch_the_repository_has() {
    let (_dir, verifier, _) = repository();
    let branch_exists = json!({ "check": "branch_exists" });

    assert_observed(
        &check(&verifier, json!("feature/refunds"), branch_exists.clone()).await,
        Pass,
        "branch feature/refunds is at",
    );
    assert_observed(
        &check(&verifier, json!("feature/payouts"), branch_exists).await,
        Fail,
        &format!("{REPO} has no branch feature/payouts"),
    );
}

#[tokio::test]
async fn merged_into_passes_only_for_a_branch_merged_into_the_target() {
    let (_dir, verifier, _) = repository();
    let merged_into = json!({ "check": "merged_into", "into": "main" });

    assert_observed(
        &check(&verifier, json!("feature/invoices"), merged_into.clone()).await,
        Pass,
        "feature/invoices is merged into main",
    );
    assert_observed(
        &check(&verifier, json!("feature/refunds"), merged_into.clone()).await,
        Fail,
        "feature/refunds isn't merged into main",
    );
    // A branch deleted after it was merged can't be told apart from one never pushed.
    assert_observed(
        &check(&verifier, json!("feature/payouts"), merged_into).await,
        Inconclusive,
        "git: ",
    );
}

#[tokio::test]
async fn file_contains_reads_the_file_as_it_is_at_a_commit() {
    let (_dir, verifier, _) = repository();
    let readme = json!({ "check": "file_contains", "path": "README.md" });

    assert_observed(
        &check(&verifier, json!("v2 invoices API"), readme.clone()).await,
        Pass,
        "README.md at HEAD contains \"v2 invoices API\"",
    );
    assert_observed(
        &check(&verifier, json!("v3"), readme).await,
        Fail,
        "README.md at HEAD doesn't contain \"v3\"",
    );
    let at_start = json!({ "check": "file_contains", "path": "README.md", "ref": "main~1" });
    assert_observed(
        &check(&verifier, json!("v2 invoices API"), at_start).await,
        Fail,
        "README.md at main~1 doesn't contain",
    );
    let refunds = json!({ "check": "file_contains", "path": "REFUNDS.md" });
    assert_observed(
        &check(&verifier, json!("refunds"), refunds).await,
        Fail,
        "REFUNDS.md doesn't exist at HEAD",
    );
    let nested = json!({ "check": "file_contains", "path": "docs/runbook.md" });
    assert_observed(
        &check(&verifier, json!("restart"), nested).await,
        Pass,
        "docs/runbook.md at HEAD contains \"restart\"",
    );
    let directory = json!({ "check": "file_contains", "path": "docs" });
    assert_observed(
        &check(&verifier, json!("restart"), directory).await,
        Fail,
        "docs isn't a file at HEAD",
    );
}

#[tokio::test]
async fn what_keeps_the_verifier_from_looking_is_inconclusive() {
    let (_dir, verifier, _) = repository();

    let unconfigured = verify(
        &verifier,
        &claim("repo:acme/payments", "under_test", json!("main")),
        json!({ "check": "branch_exists" }),
    )
    .await;
    assert_observed(
        &unconfigured,
        Inconclusive,
        "no local clone of repo:acme/payments",
    );

    let option = check(
        &verifier,
        json!("--all"),
        json!({ "check": "commit_exists" }),
    )
    .await;
    assert_observed(&option, Inconclusive, "\"--all\" isn't a git ref");

    let no_target = check(
        &verifier,
        json!("feature/invoices"),
        json!({ "check": "merged_into" }),
    )
    .await;
    assert_observed(&no_target, Inconclusive, "needs the into parameter");

    let unknown = check(&verifier, json!("main"), json!({ "check": "tag_exists" })).await;
    assert_eq!(unknown.outcome, Inconclusive, "{unknown:?}");

    let not_a_repo = tempfile::tempdir().unwrap();
    let elsewhere = GitVerifier::new([(REPO.to_owned(), not_a_repo.path().to_path_buf())]);
    let observed = check(
        &elsewhere,
        json!("main"),
        json!({ "check": "branch_exists" }),
    )
    .await;
    assert_observed(&observed, Inconclusive, "git: ");
}
