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
    pub status: Status,
    #[serde(with = "time::serde::rfc3339")]
    pub valid_from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub valid_to: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
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

/// Where a claim's source falls, decided by the channel it arrived through (ADR-0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    pub(crate) fn for_channel(channel: Channel) -> Self {
        match channel {
            Channel::Visualizer | Channel::Cli | Channel::Elicitation => SourceKind::UserStated,
            // Integrations declare their source kind once `declare_source` lands (ticket #04).
            Channel::Agent | Channel::Integration => SourceKind::AgentClaim,
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

/// Where a claim came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub actor: String,
    pub channel: Channel,
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
        let level = match level {
            "user" => ScopeLevel::User,
            "project" => ScopeLevel::Project,
            "agent" => ScopeLevel::Agent,
            "session" => ScopeLevel::Session,
            _ => return Err(invalid()),
        };
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
