//! The claim: one structured statement with its source, status and time.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use time::OffsetDateTime;

use crate::caller::Channel;

/// One structured statement about the world, as it stands in memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub id: ClaimId,
    pub scope: Scope,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub text: String,
    pub source: Source,
    /// What backs the claim. Written only when there is some, so claims without evidence keep
    /// the content their journal entries were hashed with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    pub status: Status,
    #[serde(with = "time::serde::rfc3339")]
    pub valid_from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub valid_to: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
}

impl Claim {
    /// How far the claim is trusted on its own: its source prior, or its strongest evidence if
    /// that is stronger.
    pub fn trust(&self) -> f64 {
        self.evidence
            .iter()
            .map(|evidence| evidence.kind.strength())
            .fold(self.source.kind.prior(), f64::max)
    }
}

/// A claim identifier: `clm_` followed by a ULID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClaimId(String);

impl ClaimId {
    pub(crate) fn generate(at: OffsetDateTime) -> Self {
        ClaimId(format!("clm_{}", ulid::Ulid::from_datetime(at.into())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClaimId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a claim's source falls, decided by the channel it arrived through (ADR-0009). Declared
/// most trusted first, so the greater of two kinds is the less trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    System,
    UserStated,
    ToolResult,
    AgentClaim,
    Document,
    OtherAgent,
    Web,
}

impl SourceKind {
    fn as_str(self) -> &'static str {
        match self {
            SourceKind::System => "system",
            SourceKind::UserStated => "user_stated",
            SourceKind::ToolResult => "tool_result",
            SourceKind::AgentClaim => "agent_claim",
            SourceKind::Document => "document",
            SourceKind::OtherAgent => "other_agent",
            SourceKind::Web => "web",
        }
    }

    /// The starting trust a claim gets from its source kind alone.
    pub fn prior(self) -> f64 {
        match self {
            SourceKind::System => 0.80,
            SourceKind::UserStated => 0.70,
            SourceKind::ToolResult => 0.60,
            SourceKind::AgentClaim | SourceKind::Document => 0.30,
            SourceKind::OtherAgent => 0.25,
            SourceKind::Web => 0.15,
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a claim came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub actor: String,
    pub channel: Channel,
}

/// Something observable that backs a claim, and when it was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// `evd_` followed by a ULID.
    pub id: String,
    #[serde(flatten)]
    pub kind: EvidenceKind,
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
}

impl Evidence {
    pub(crate) fn observed(kind: EvidenceKind, at: OffsetDateTime) -> Self {
        Evidence {
            id: format!("evd_{}", ulid::Ulid::from_datetime(at.into())),
            kind,
            observed_at: at,
        }
    }
}

/// What a piece of evidence is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A person confirmed the claim through the visualizer or the command line.
    HumanConfirmation,
    /// An agent reported the user's exact words about the user's own preferences or identity,
    /// holding relay_user.
    RelayedUserStatement { quote: String },
}

impl EvidenceKind {
    /// How strongly the evidence attests the claim (claims-and-integrity.md, section 5).
    pub fn strength(&self) -> f64 {
        match self {
            EvidenceKind::HumanConfirmation => 0.90,
            EvidenceKind::RelayedUserStatement { .. } => 0.80,
        }
    }
}

/// Where a claim stands in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Proposed,
    Verified,
    Contested,
    Superseded,
    Retracted,
    Quarantined,
}

/// The visibility boundary a claim lives in within a tenant, written `project:billing-api`, or
/// `org` for the tenant-wide scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Scope {
    pub level: ScopeLevel,
    /// The user, project, agent or session id. Org scope has none.
    pub id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeLevel {
    Org,
    User,
    Project,
    Agent,
    Session,
}

impl Scope {
    pub fn new(level: ScopeLevel, id: &str) -> Self {
        Scope {
            level,
            id: Some(id.to_owned()),
        }
    }

    pub fn org() -> Self {
        Scope {
            level: ScopeLevel::Org,
            id: None,
        }
    }
}

impl ScopeLevel {
    const ALL: [ScopeLevel; 5] = [
        ScopeLevel::Org,
        ScopeLevel::User,
        ScopeLevel::Project,
        ScopeLevel::Agent,
        ScopeLevel::Session,
    ];

    fn as_str(self) -> &'static str {
        match self {
            ScopeLevel::Org => "org",
            ScopeLevel::User => "user",
            ScopeLevel::Project => "project",
            ScopeLevel::Agent => "agent",
            ScopeLevel::Session => "session",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.id {
            Some(id) => write!(f, "{}:{id}", self.level.as_str()),
            None => f.write_str(self.level.as_str()),
        }
    }
}

impl FromStr for Scope {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("{text:?} is not a scope like project:billing-api or org");
        if text == "org" {
            return Ok(Scope::org());
        }
        let (level, id) = text.split_once(':').ok_or_else(invalid)?;
        let level = ScopeLevel::ALL
            .into_iter()
            .find(|candidate| *candidate != ScopeLevel::Org && candidate.as_str() == level)
            .ok_or_else(invalid)?;
        if id.is_empty() {
            return Err(invalid());
        }
        Ok(Scope::new(level, id))
    }
}

impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}
