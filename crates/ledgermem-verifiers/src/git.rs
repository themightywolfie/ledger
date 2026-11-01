//! The `git` verifier: checks commits, branches, merges and file contents in the local clone of
//! the repository a claim is about, through the `git` command.
//!
//! | `check` | Parameters | Passes when |
//! | --- | --- | --- |
//! | `commit_exists` | `ref` (default: the claim's value) | `ref` names a commit |
//! | `branch_exists` | `ref` (default: the claim's value) | the repository has branch `ref` |
//! | `merged_into` | `ref` (default: the claim's value), `into` | `ref` is merged into `into` |
//! | `file_contains` | `path`, `pattern` (default: the claim's value), `ref` (default: `HEAD`) | `path` at `ref` contains `pattern` |
//!
//! The claim's subject names the repository, such as `repo:acme/billing-api`, and the verifier
//! knows which local clone each subject is.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use ledgermem_core::{Check, Observation, Verifier, Verifying};
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{NOTHING_TO_FIND, TIMEOUT, contains, needs, unsupported};

/// Checks claims about the repositories it knows the local clones of.
#[derive(Debug, Clone, Default)]
pub struct GitVerifier {
    /// Each repository subject, as Ledger keeps it, and its local clone.
    repos: BTreeMap<String, PathBuf>,
}

impl GitVerifier {
    /// A verifier for `repos`: each a subject such as `repo:acme/billing-api`, and the directory
    /// of its local clone.
    pub fn new(repos: impl IntoIterator<Item = (String, PathBuf)>) -> Self {
        GitVerifier {
            repos: repos
                .into_iter()
                .map(|(subject, clone)| (subject.trim().to_lowercase(), clone))
                .collect(),
        }
    }

    /// What the repository says, or why it couldn't be asked.
    async fn observe(&self, check: Check<'_>) -> Result<Observation, String> {
        let subject = &check.claim.subject;
        let repo = self
            .repos
            .get(subject)
            .ok_or_else(|| format!("no local clone of {subject} is configured"))?;
        let value = || check.expand("{value}");
        match check.param("check").as_deref() {
            Some("commit_exists") => {
                let reference = reference(&check, "ref", value)?;
                Ok(match commit(repo, &reference).await? {
                    Some(sha) => Observation::pass(
                        json!(sha),
                        format!("{reference} is commit {sha} in {subject}"),
                    ),
                    None => Observation::fail(
                        Value::Null,
                        format!("{subject} has no commit {reference}"),
                    ),
                })
            }
            Some("branch_exists") => {
                let branch = reference(&check, "ref", value)?;
                let output = git(
                    repo,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{branch}"),
                    ],
                )
                .await?;
                Ok(match answer(&output)? {
                    true => {
                        let sha = stdout(&output);
                        Observation::pass(json!(sha), format!("branch {branch} is at {sha}"))
                    }
                    false => {
                        Observation::fail(Value::Null, format!("{subject} has no branch {branch}"))
                    }
                })
            }
            Some("merged_into") => {
                let merged = reference(&check, "ref", value)?;
                let into = reference(&check, "into", String::new)?;
                let output = git(repo, &["merge-base", "--is-ancestor", &merged, &into]).await?;
                Ok(match answer(&output)? {
                    true => {
                        Observation::pass(json!(true), format!("{merged} is merged into {into}"))
                    }
                    false => Observation::fail(
                        json!(false),
                        format!("{merged} isn't merged into {into}"),
                    ),
                })
            }
            Some("file_contains") => {
                let path = check
                    .param("path")
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| needs("path"))?;
                let pattern = check.param("pattern").unwrap_or_else(value);
                if pattern.is_empty() {
                    return Err(NOTHING_TO_FIND.into());
                }
                let at = reference(&check, "ref", || "HEAD".to_owned())?;
                let sha = commit(repo, &at)
                    .await?
                    .ok_or_else(|| format!("{subject} has no commit {at}"))?;
                // What the commit has at `path`: a file, a directory, or nothing.
                let listed = git(repo, &["ls-tree", &sha, "--", &path]).await?;
                if !listed.status.success() {
                    return Err(said(&listed));
                }
                match stdout(&listed).split_whitespace().nth(1) {
                    None => {
                        return Ok(Observation::fail(
                            json!(false),
                            format!("{path} doesn't exist at {at}"),
                        ));
                    }
                    Some("blob") => {}
                    Some(_) => {
                        return Ok(Observation::fail(
                            json!(false),
                            format!("{path} isn't a file at {at}"),
                        ));
                    }
                }
                let output = git(repo, &["cat-file", "blob", &format!("{sha}:{path}")]).await?;
                if !output.status.success() {
                    return Err(said(&output));
                }
                Ok(if contains(&output.stdout, pattern.as_bytes()) {
                    Observation::pass(json!(true), format!("{path} at {at} contains {pattern:?}"))
                } else {
                    Observation::fail(
                        json!(false),
                        format!("{path} at {at} doesn't contain {pattern:?}"),
                    )
                })
            }
            other => Err(unsupported(
                "git",
                "commit_exists, branch_exists, merged_into or file_contains",
                other,
            )),
        }
    }
}

impl Verifier for GitVerifier {
    fn verify<'a>(&'a self, check: Check<'a>) -> Verifying<'a> {
        Box::pin(async move {
            // One timeout for the whole check, however many git commands it takes. A command
            // still running when it expires is killed.
            match tokio::time::timeout(TIMEOUT, self.observe(check)).await {
                Ok(observed) => observed.unwrap_or_else(Observation::inconclusive),
                Err(_) => Observation::inconclusive(format!(
                    "git took longer than {}s",
                    TIMEOUT.as_secs()
                )),
            }
        })
    }
}

/// The ref the parameter `name` names, or `default` when the policy doesn't set it. One that
/// could be read as an option is refused.
fn reference(
    check: &Check<'_>,
    name: &str,
    default: impl FnOnce() -> String,
) -> Result<String, String> {
    let reference = check.param(name).unwrap_or_else(default);
    if reference.is_empty() {
        return Err(needs(name));
    }
    if reference.starts_with('-') {
        return Err(format!("{reference:?} isn't a git ref"));
    }
    Ok(reference)
}

/// The full id of the commit `reference` names, or None when it names none.
async fn commit(repo: &Path, reference: &str) -> Result<Option<String>, String> {
    let output = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ],
    )
    .await?;
    Ok(answer(&output)?.then(|| stdout(&output)))
}

/// Runs git in `repo`.
async fn git(repo: &Path, args: &[&str]) -> Result<Output, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    match command.output().await {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Err("git isn't installed on this server".into())
        }
        Err(error) => Err(format!("git couldn't run: {error}")),
    }
}

/// A yes-or-no git command's answer: exit 0 for yes and 1 for no. Anything else is an error,
/// such as a directory that isn't a repository.
fn answer(output: &Output) -> Result<bool, String> {
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(said(output)),
    }
}

/// What git said when a command failed.
fn said(output: &Output) -> String {
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.lines().next().unwrap_or("").trim();
    format!("git: {}", said.strip_prefix("fatal: ").unwrap_or(said))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
