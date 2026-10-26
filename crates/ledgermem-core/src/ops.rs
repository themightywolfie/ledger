//! Requests the engine accepts and the outcomes it returns. Surfaces serialize these as they are.
//!
//! Requests refuse fields they don't know, so an argument this version doesn't support is an
//! error rather than silently ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::claim::{ClaimId, Scope, Status};

/// Propose claims for memory.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Remember {
    /// The claims to propose: one subject, one predicate and one value each.
    pub claims: Vec<ClaimInput>,
}

/// One claim as a caller writes it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ClaimInput {
    /// The thing the claim is about, as `kind:name`, such as `service:billing-api`.
    pub subject: String,
    /// The property or relationship asserted, such as `deployed_version`.
    pub predicate: String,
    /// What the predicate is for the subject: a JSON scalar.
    pub value: Value,
    /// A one-sentence natural-language rendering of the claim.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RememberOutcome {
    /// One result per claim, in the order they were written.
    pub results: Vec<ClaimResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClaimResult {
    pub claim_id: ClaimId,
    pub status: Status,
    pub trust: f64,
}

/// Recall what is known about a subject or question.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Recall {
    /// The question or keywords to recall by.
    pub query: String,
    /// The most claims to return. Defaults to 10.
    pub limit: Option<usize>,
}

/// The time a read is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadIntent {
    Current,
}

/// What a read returns.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadPack {
    pub intent: ReadIntent,
    pub answer_set: Vec<RecalledClaim>,
}

/// One claim in an answer set.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecalledClaim {
    pub claim_id: ClaimId,
    pub scope: Scope,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub text: String,
    pub status: Status,
    pub trust: f64,
    /// How long ago Ledger learned the claim, such as `38m`.
    pub age: String,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
}

/// Read one scope's journal.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalQuery {
    pub scope: Scope,
}
