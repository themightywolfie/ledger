//! Requests the engine accepts and the outcomes it returns. Surfaces serialize these as they are.
//!
//! Requests refuse fields they don't know, so an argument this version doesn't support is an
//! error rather than silently ignored.

use std::fmt;

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

/// What a reindex rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReindexReport {
    /// How many journals were replayed.
    pub scopes: u64,
    /// How many entries they held.
    pub entries: u64,
    /// How many claims the rebuilt projections hold.
    pub claims: u64,
}

/// Whether every journal in a tenant still verifies.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChainReport {
    /// True when no scope has a broken link.
    pub ok: bool,
    /// One report per scope with a journal, in scope order.
    pub scopes: Vec<ScopeVerification>,
}

/// One scope's journal, as verification found it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeVerification {
    /// The scope as stored, written `level:id` or `org`.
    pub scope: String,
    /// How many entries the journal holds.
    pub entries: u64,
    /// The last entry's hash.
    pub head_hash: Option<String>,
    /// The first entry that doesn't verify, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broken_at: Option<BrokenLink>,
}

/// The first entry in a journal that doesn't verify, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrokenLink {
    pub seq: u64,
    pub problem: ChainProblem,
}

/// Why an entry doesn't verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainProblem {
    /// The entry is missing: the journal skips from the entry before it to a later one.
    MissingEntry,
    /// The entry's content no longer matches its content hash.
    ContentHashMismatch,
    /// The entry doesn't link to the hash of the entry before it, so an earlier entry was
    /// replaced or the entry was moved.
    PrevHashMismatch,
    /// A header field, such as the actor or the time, no longer matches the entry's hash.
    EntryHashMismatch,
}

impl fmt::Display for ChainProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ChainProblem::MissingEntry => "the entry is missing",
            ChainProblem::ContentHashMismatch => "its content no longer matches its content hash",
            ChainProblem::PrevHashMismatch => {
                "it no longer links to the entry before it, so an earlier entry was replaced"
            }
            ChainProblem::EntryHashMismatch => {
                "a header field, such as the actor or the time, no longer matches its hash"
            }
        })
    }
}
