//! Requests the engine accepts and the outcomes it returns. Surfaces serialize these as they are.
//!
//! Requests refuse fields they don't know, so an argument this version doesn't support is an
//! error rather than silently ignored.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::claim::{ClaimId, Scope, SourceKind, Status};

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
    /// For something the user told you about themselves: their exact words.
    pub relayed_user_statement: Option<RelayedUserStatement>,
    /// Where you read the claim, if it wasn't from your own tools or the user: a web page, a
    /// document or another agent. It only ever lowers trust.
    pub reported_source: Option<ReportedSource>,
    /// Where an integration with the declare_source permission says the claim came from, as
    /// `{ "kind": "tool_result" }`. Anyone else's is ignored, whatever it holds, because a claim's
    /// source kind comes from the channel and key that wrote it (ADR-0009). Not offered to models.
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub source: Option<Value>,
}

impl ClaimInput {
    /// A claim in the caller's own words, with no relayed or reported source.
    pub fn new(
        subject: impl Into<String>,
        predicate: impl Into<String>,
        value: Value,
        text: impl Into<String>,
    ) -> Self {
        ClaimInput {
            subject: subject.into(),
            predicate: predicate.into(),
            value,
            text: text.into(),
            relayed_user_statement: None,
            reported_source: None,
            source: None,
        }
    }
}

/// The user's own words, as an agent relays them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RelayedUserStatement {
    /// Exactly what the user said, such as "I'm vegetarian".
    pub quote: String,
}

/// Where a caller says it read a claim, which can make its source kind less trusted but never
/// more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum ReportedSource {
    Web,
    Document,
    OtherAgent,
}

impl From<ReportedSource> for SourceKind {
    fn from(reported: ReportedSource) -> Self {
        match reported {
            ReportedSource::Web => SourceKind::Web,
            ReportedSource::Document => SourceKind::Document,
            ReportedSource::OtherAgent => SourceKind::OtherAgent,
        }
    }
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
    /// Advice on the claim as written: a declared predicate to use instead of an undeclared one,
    /// or why the write gate treated the claim as it did, such as why it was quarantined.
    pub hints: Vec<String>,
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

/// Apply a policy document to the tenant, or to one project as an overlay on the tenant's.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyPolicy {
    /// The policy document, as YAML or JSON text.
    pub document: String,
    /// The project to overlay. Without one, the document becomes the tenant policy.
    pub project: Option<String>,
}

/// The policy version an apply created.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PolicyApplied {
    pub policy_version: String,
    /// `org`, or the project the version overlays.
    pub scope: Scope,
    /// How many predicates the version declares, counting those of the packs it extends.
    pub predicates: u64,
    /// How many aliases it declares.
    pub aliases: u64,
}

/// Read the policy in effect.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyQuery {
    /// Include this project's overlay. Without one, the tenant policy alone.
    pub project: Option<String>,
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
