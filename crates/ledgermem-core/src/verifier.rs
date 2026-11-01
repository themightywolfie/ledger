//! The verifier port: checks a claim against a real system without changing it
//! (claims-and-integrity.md, sections 4 and 5).
//!
//! The engine asks the verifier a claim's predicate declares, and records what it found as
//! evidence. A verifier never fails: a timeout, a missing tool or anything else that keeps it from
//! looking comes back inconclusive, so a flaky dependency can never promote a claim.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use crate::claim::{Claim, Evidence, EvidenceKind};
use crate::journal;
use crate::policy::VerifierName;

/// Checks claims of one shape against a real system, read-only.
pub trait Verifier: Send + Sync {
    /// What the system says about the claim `check` names. Anything that keeps the verifier from
    /// looking, a timeout included, is an inconclusive observation rather than an error.
    fn verify<'a>(&'a self, check: Check<'a>) -> Verifying<'a>;
}

/// What a verifier returns: the observation, once it has looked.
pub type Verifying<'a> = Pin<Box<dyn Future<Output = Observation> + Send + 'a>>;

/// What a verifier is asked: whether a claim holds, with the parameters its predicate declares.
#[derive(Debug, Clone, Copy)]
pub struct Check<'a> {
    pub claim: &'a Claim,
    pub params: &'a Map<String, Value>,
}

/// What a verifier found.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub outcome: Outcome,
    /// What the verifier saw that decided the outcome, such as the version a system reports, or
    /// null when it couldn't look.
    pub observed: Value,
    /// What it found, in a sentence for people, such as "3 of 412 tests fail".
    pub summary: String,
}

/// Whether a verifier found that a claim holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The claim holds.
    Pass,
    /// The verifier observed the opposite, which refutes the claim.
    Fail,
    /// The verifier couldn't tell, so nothing changes.
    Inconclusive,
}

/// The verifiers a ledger can check claims with, under the names policies use for them.
#[derive(Clone, Default)]
pub struct Verifiers(BTreeMap<VerifierName, Arc<dyn Verifier>>);

/// A verifier that answers as a test scripts it, by the subject of the claim it is asked about. A
/// subject with nothing scripted is inconclusive.
#[derive(Debug, Default)]
pub struct ScriptedVerifier {
    script: Mutex<BTreeMap<String, Observation>>,
}

impl Check<'_> {
    /// The parameter `name` as text, with the claim filled into it as a template, if it is set.
    pub fn param(&self, name: &str) -> Option<String> {
        self.params
            .get(name)
            .map(|value| self.expand(&as_text(value)))
    }

    /// `template` with `{subject_name}` (the part of the subject after `kind:`), `{subject}`,
    /// `{value}` and `{predicate}` filled in from the claim. What fills a placeholder is never
    /// read as a placeholder itself.
    pub fn expand(&self, template: &str) -> String {
        let claim = self.claim;
        let subject_name = claim
            .subject
            .split_once(':')
            .map_or(claim.subject.as_str(), |(_, name)| name);
        let value = as_text(&claim.value);
        let fields = [
            ("{subject_name}", subject_name),
            ("{subject}", claim.subject.as_str()),
            ("{value}", value.as_str()),
            ("{predicate}", claim.predicate.as_str()),
        ];
        let mut expanded = String::with_capacity(template.len());
        let mut rest = template;
        'scan: while let Some(next) = rest.chars().next() {
            for (placeholder, with) in fields {
                if let Some(after) = rest.strip_prefix(placeholder) {
                    expanded.push_str(with);
                    rest = after;
                    continue 'scan;
                }
            }
            expanded.push(next);
            rest = &rest[next.len_utf8()..];
        }
        expanded
    }
}

impl Observation {
    pub fn pass(observed: Value, summary: impl Into<String>) -> Self {
        Self::new(Outcome::Pass, observed, summary)
    }

    pub fn fail(observed: Value, summary: impl Into<String>) -> Self {
        Self::new(Outcome::Fail, observed, summary)
    }

    /// The verifier couldn't look, and `why`.
    pub fn inconclusive(why: impl Into<String>) -> Self {
        Self::new(Outcome::Inconclusive, Value::Null, why)
    }

    pub fn new(outcome: Outcome, observed: Value, summary: impl Into<String>) -> Self {
        Observation {
            outcome,
            observed,
            summary: summary.into(),
        }
    }

    /// The observation as evidence that `verifier` observed at `at`, hashed so the evidence fixes
    /// what was seen.
    pub(crate) fn into_evidence(
        self,
        verifier: VerifierName,
        system_of_record: bool,
        at: OffsetDateTime,
    ) -> Evidence {
        let seen = json!({ "observed": self.observed, "summary": self.summary });
        let kind = EvidenceKind::Verifier {
            verifier,
            outcome: self.outcome,
            system_of_record,
            hash: journal::content_hash(&seen),
            observed: self.observed,
            summary: self.summary,
        };
        Evidence::observed(kind, at)
    }
}

impl Verifiers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `verifier` under `name`, in place of any verifier already there.
    pub fn with(mut self, name: VerifierName, verifier: Arc<dyn Verifier>) -> Self {
        self.0.insert(name, verifier);
        self
    }

    pub(crate) fn get(&self, name: &VerifierName) -> Option<&dyn Verifier> {
        self.0.get(name).map(Arc::as_ref)
    }
}

impl ScriptedVerifier {
    /// Answers `observation` about claims on `subject`, written as Ledger keeps it, from now on.
    pub fn script(&self, subject: &str, observation: Observation) {
        self.script
            .lock()
            .expect("script lock")
            .insert(subject.to_owned(), observation);
    }
}

impl Verifier for ScriptedVerifier {
    fn verify<'a>(&'a self, check: Check<'a>) -> Verifying<'a> {
        let subject = &check.claim.subject;
        let observation = self
            .script
            .lock()
            .expect("script lock")
            .get(subject)
            .cloned()
            .unwrap_or_else(|| {
                Observation::inconclusive(format!("nothing is scripted for {subject}"))
            });
        Box::pin(std::future::ready(observation))
    }
}

/// A JSON value as template text: a string as itself, anything else as JSON.
fn as_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
