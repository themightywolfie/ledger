//! Ledger's built-in verifiers. Each checks claims of one shape against a real system without
//! changing it, and comes back inconclusive, never with an error, when it can't look. They run on
//! a Tokio runtime.
//!
//! A built-in check finds whether the thing a claim names is there: a file, some text in it, a
//! commit or a branch. What it looks for comes from the parameters the claim's predicate
//! declares, each a template the claim fills in (contracts.md, section 5), and by default from
//! the claim's value. The test_run verifier checks a repository's JUnit report instead. The http,
//! github and exec verifiers arrive with ticket #08.

mod file;
mod git;
mod test_run;

pub use file::FileVerifier;
pub use git::GitVerifier;
pub use test_run::{Rerun, TestRunRepo, TestRunVerifier};

use std::time::Duration;

/// How long a built-in verifier may take before it gives up, inconclusive.
const TIMEOUT: Duration = Duration::from_secs(2);

/// Runs `work`, which blocks, away from the async runtime, giving up after the timeout. The
/// error says why there is no result, naming the work as `what`.
async fn blocking<T: Send + 'static>(
    what: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    match tokio::time::timeout(TIMEOUT, tokio::task::spawn_blocking(work)).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(format!("{what} failed: {error}")),
        Err(_) => Err(format!("{what} took longer than {}s", TIMEOUT.as_secs())),
    }
}

/// Why a `contains` check with empty text has nothing to look for.
const NOTHING_TO_FIND: &str = "there is no text to look for";

/// Why a check can't run without the parameter `name`.
fn needs(name: &str) -> String {
    format!("this check needs the {name} parameter")
}

/// Why `verifier` can't run the check a policy names: it runs only `checks`.
fn unsupported(verifier: &str, checks: &str, named: Option<&str>) -> String {
    let named = named.map_or_else(|| "nothing".to_owned(), |named| format!("{named:?}"));
    format!("the {verifier} verifier checks {checks}, not {named}")
}

/// Whether `needle` occurs in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
