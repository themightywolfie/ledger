//! Requests the engine accepts and the outcomes it returns. Surfaces serialize these as they are.
//!
//! Requests refuse fields they don't know, so an argument this version doesn't support is an
//! error rather than silently ignored.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::claim::{ClaimId, Evidence, RetractionReason, Scope, SourceKind, Status};
use crate::trust::Trust;
use crate::verifier::Outcome;

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
    /// What you observed through your tools that backs the claim. It is recorded, but a claim
    /// about your own work stays proposed until its verifier checks it.
    #[serde(default)]
    pub evidence: Vec<EvidenceInput>,
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
            evidence: Vec::new(),
            relayed_user_statement: None,
            reported_source: None,
            source: None,
        }
    }
}

/// Something a caller observed through one of its own tools.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct EvidenceInput {
    /// What kind of evidence it is. Only `tool_result`, the default.
    #[serde(default)]
    pub kind: EvidenceInputKind,
    /// The tool you observed it through, such as `kubectl.get_deployment`.
    pub tool: String,
    /// What the tool looked at, such as `deployment/billing-api`.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// What it returned that bears on the claim, such as `image: billing-api:2.14.0`.
    pub excerpt: Option<String>,
    /// When you observed it, in RFC 3339. Defaults to now, and can't be later.
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub observed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum EvidenceInputKind {
    /// The output of a tool you called.
    #[default]
    ToolResult,
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
    /// Why the claim was retracted, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<RetractionReason>,
    /// Whether the predicate's verifier checked the claim as it was written.
    pub verification: Verification,
    /// Verified, and scored at or above the trusted threshold.
    pub trusted: bool,
    /// The claim's trust as written, with its components.
    pub trust: Trust,
    /// Advice on the claim as written: a declared predicate to use instead of an undeclared one,
    /// or why the write gate treated the claim as it did, such as why it was quarantined.
    pub hints: Vec<String>,
}

/// Whether a claim's verifier checked it as it was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    /// Nothing checked it: its predicate declares no verifier this server has, or the write gate
    /// quarantined it.
    #[serde(rename = "none")]
    Unchecked,
    /// Its verifier ran inline, and the claim's status shows the outcome.
    Done,
    /// Its verifier is slow, so it checks the claim in the background, and the claim stays as
    /// written until it has.
    Pending,
}

/// What attesting found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestedOutcome {
    /// The verifier found that the claim holds.
    Pass,
    /// The verifier observed the opposite, which refutes the claim.
    Fail,
    /// Nothing could tell, so nothing changed.
    Inconclusive,
    /// The verifier is slow, so it checks the claim in the background.
    Pending,
}

impl From<Outcome> for AttestedOutcome {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Pass => AttestedOutcome::Pass,
            Outcome::Fail => AttestedOutcome::Fail,
            Outcome::Inconclusive => AttestedOutcome::Inconclusive,
        }
    }
}

/// What a run of the job runner did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct JobsRun {
    /// How many jobs it ran.
    pub ran: u64,
    /// Why each job that failed did. A failed job stays leased, and runs again once its lease
    /// runs out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

/// Attach evidence to a claim, or ask Ledger to check it now against its verifier.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Attest {
    /// The claim to attest, as `memory_remember` or `memory_recall` returned it.
    pub claim_id: ClaimId,
    /// What you observed through your tools that bears on the claim.
    #[serde(default)]
    pub evidence: Vec<EvidenceInput>,
    /// Whether to check the claim against its predicate's verifier now. Defaults to true.
    #[serde(default = "verify_now_by_default")]
    pub verify_now: bool,
}

fn verify_now_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AttestOutcome {
    pub claim_id: ClaimId,
    /// What checking the claim found. Inconclusive when nothing could check it, and pending
    /// while a slow verifier checks it in the background.
    pub outcome: AttestedOutcome,
    pub status: Status,
    /// Why the claim was retracted, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<RetractionReason>,
    /// Verified, and scored at or above the trusted threshold.
    pub trusted: bool,
    /// The claim's trust now, with its components.
    pub trust: Trust,
    /// The evidence this call attached, each piece hashed and timestamped.
    pub evidence: Vec<Evidence>,
    /// Why the claim didn't pass, or why nothing checked it.
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
    /// Whether you are about to act on the answer in a way that is costly to get wrong. Ledger
    /// then abstains rather than offer an answer that is stale or unverified.
    #[serde(default)]
    pub high_stakes: bool,
}

impl Recall {
    /// An ordinary read of `query`, returning up to the default number of claims.
    pub fn new(query: impl Into<String>) -> Self {
        Recall {
            query: query.into(),
            limit: None,
            high_stakes: false,
        }
    }
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
    /// Every candidate, best match first, each labelled with how far it can be relied on.
    pub answer_set: Vec<RecalledClaim>,
    /// The stale claims in the answer set whose re-verification is queued, so a later read may
    /// find them fresh.
    pub stale: Vec<ClaimId>,
    pub guidance: Guidance,
}

/// How far a read's best answer, the first in its answer set, can be relied on
/// (claims-and-integrity.md, section 9).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Guidance {
    /// The verdict, for an adapter to act on.
    pub code: GuidanceCode,
    /// What the code means for whoever reads the answer.
    pub text: String,
}

/// A read's verdict on its best answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidanceCode {
    /// The best answer is trusted and fresh.
    Reliable,
    /// The best answer isn't trusted: it is proposed, or verified but below the trusted threshold.
    Unverified,
    /// The best answer is verified but past its freshness interval.
    Stale,
    /// The best answer is contested, or the read is high-stakes and the best answer is stale or
    /// unverified. Every candidate is returned, and none is picked.
    Unresolved,
    /// There is no answer.
    #[serde(rename = "none")]
    NoAnswer,
}

impl GuidanceCode {
    fn as_str(self) -> &'static str {
        match self {
            GuidanceCode::Reliable => "reliable",
            GuidanceCode::Unverified => "unverified",
            GuidanceCode::Stale => "stale",
            GuidanceCode::Unresolved => "unresolved",
            GuidanceCode::NoAnswer => "none",
        }
    }
}

impl fmt::Display for GuidanceCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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
    /// Verified, and scored at or above the trusted threshold at the time of the read.
    pub trusted: bool,
    /// The claim's trust at the time of the read.
    pub trust: Trust,
    /// How long ago Ledger learned the claim, such as `38m`.
    pub age: String,
    /// Longer since it was last attested than its predicate's freshness allows.
    pub stale: bool,
    /// Trusted and fresh, so it may be placed in a model's instructions. Anything else is only
    /// labelled data.
    pub injectable: bool,
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
