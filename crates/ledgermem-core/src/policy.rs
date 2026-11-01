//! The policy: the predicates a tenant declares, the aliases of its subjects and its resolution
//! policy (contracts.md, section 5).
//!
//! An operator applies a policy document to the tenant, or to one project as an overlay on the
//! tenant's. Each applied document is a policy version, journaled together with the predicate
//! packs it extends as they were when it was applied, so replaying the journal rebuilds the same
//! policy even after a later build changes a pack. Aliases are tenant-wide, so every scope
//! resolves a subject the same way.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use unicode_normalization::UnicodeNormalization;

use crate::claim::{Scope, ScopeLevel};
use crate::error::{Error, Result};

/// The document format version this build reads.
const DOCUMENT_VERSION: u32 = 1;

/// The predicate packs this build ships, by name.
const PACKS: [(&str, &str); 2] = [
    (
        "software-delivery",
        include_str!("../policies/software-delivery.yaml"),
    ),
    (
        "personal-assistant",
        include_str!("../policies/personal-assistant.yaml"),
    ),
];

/// How alike an undeclared predicate and a declared one must be, by Jaro-Winkler similarity, for
/// remember to suggest the declared one.
const HINT_SIMILARITY: f64 = 0.84;

/// The verifiers a predicate can name without a server-config instance.
const BUILT_IN_VERIFIERS: [&str; 6] = ["test_run", "git", "github", "file", "human", "llm_judge"];

/// A policy document as an operator writes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDocument {
    /// The document format version. Always 1.
    pub version: u32,
    /// Predicate packs to start from, in order. The document's own declarations replace theirs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<String>,
    #[serde(default, skip_serializing_if = "Subjects::is_empty")]
    pub subjects: Subjects,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub predicates: BTreeMap<String, Predicate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ResolutionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<TrustPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<FreshnessIntervals>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subjects {
    /// Each subject, with the other names that refer to it.
    #[serde(default)]
    pub aliases: BTreeMap<String, Vec<String>>,
}

/// How a predicate behaves. Every field has the default an undeclared predicate gets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    #[serde(default)]
    pub cardinality: Cardinality,
    /// Without one, values compare as identical JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_type: Option<ValueType>,
    #[serde(default)]
    pub freshness: FreshnessClass,
    /// Overrides the freshness class's re-verification interval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverify_after: Option<Interval>,
    /// The verifier whose evidence beats any other source's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_of_record: Option<VerifierName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier: Option<VerifierRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_claims: Option<AgentClaims>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<Authority>,
    /// The value tells agents what to do, so low-trust sources can't set it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub instruction: bool,
}

/// Whether a predicate holds one value at a time or several at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    #[default]
    Single,
    Multi,
}

/// What a predicate's values are, which decides how they are checked and compared.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Text,
    Number,
    Boolean,
    Datetime,
    Version,
    Subject,
    /// One of a fixed set of members.
    Enum(Vec<String>),
}

/// How quickly a predicate's claims go stale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessClass {
    Stable,
    #[default]
    Slow,
    Volatile,
}

/// A verifier and the parameters it checks a claim with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierRef {
    #[serde(rename = "use")]
    pub verifier: VerifierName,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

/// A built-in verifier, or an `http.<name>` or `exec.<name>` instance from the server config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VerifierName(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentClaims {
    /// An agent's claim without passing evidence never wins a resolution.
    RequireEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// The user's own statements settle the predicate.
    User,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionPolicy {
    /// How far apart two trust scores must be for the higher to win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_margin: Option<f64>,
    /// What happens when no rule decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_unresolved: Option<OnUnresolved>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnUnresolved {
    Contested,
    AskUser,
    PreferNewer,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustPolicy {
    /// The score at or above which a claim is trusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_threshold: Option<f64>,
}

/// The re-verification interval of each freshness class that has one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessIntervals {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow: Option<Interval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volatile: Option<Interval>,
}

/// A whole number of seconds, minutes, hours or days: `45s`, `30m`, `1h` or `90d`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Interval(String);

/// One applied policy document: what a `policy_applied` entry records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyVersion {
    /// `pol_` followed by a ULID.
    pub id: String,
    /// `org` for the tenant policy, or the project it overlays.
    pub scope: Scope,
    pub document: PolicyDocument,
    /// The packs the document extends, as they were when it was applied.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub packs: BTreeMap<String, PolicyDocument>,
}

/// The policy in effect: the tenant policy, with a project's overlay on top when there is one.
/// Anything no version sets takes its default.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Policy {
    /// The policy versions in effect, the tenant's first.
    pub versions: Vec<AppliedVersion>,
    /// The predicate packs in effect, in the order they were extended.
    pub packs: Vec<String>,
    pub subjects: Subjects,
    pub predicates: BTreeMap<String, Predicate>,
    pub resolution: ResolutionPolicy,
    pub trust: TrustPolicy,
    pub freshness: FreshnessIntervals,
    /// Each alias, and the subject it names.
    #[serde(skip)]
    subject_of: BTreeMap<String, String>,
}

/// A policy version in effect, and the scope it was applied to.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppliedVersion {
    pub id: String,
    pub scope: Scope,
}

/// A claim's subject and value as Ledger keeps them, and advice for whoever wrote it.
pub(crate) struct Admitted {
    pub subject: String,
    pub value: Value,
    pub hints: Vec<String>,
}

/// A predicate pack this build ships.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PredicatePack {
    pub name: String,
    pub predicates: BTreeMap<String, Predicate>,
}

impl PolicyDocument {
    /// Parses and validates a policy document written as YAML or JSON. An invalid document is
    /// rejected whole, naming the path of the first thing wrong with it.
    pub fn from_yaml(text: &str) -> Result<Self> {
        // Read the YAML into JSON's data model first, where an enum value type is the map
        // `{ enum: [...] }` rather than a YAML tag.
        let value: Value =
            serde_yaml::from_str(text).map_err(|error| invalid_policy(".", error))?;
        if value.is_null() {
            return Err(invalid_policy(".", "the document is empty"));
        }
        let document: PolicyDocument = serde_path_to_error::deserialize(value)
            .map_err(|error| invalid_policy(&error.path().to_string(), error.inner()))?;
        document.validate()?;
        Ok(document)
    }

    fn validate(&self) -> Result<()> {
        if self.version != DOCUMENT_VERSION {
            return Err(invalid_policy(
                "version",
                format!(
                    "this ledgermem reads version {DOCUMENT_VERSION} policy documents, not \
                     version {}",
                    self.version
                ),
            ));
        }
        for (index, name) in self.extends.iter().enumerate() {
            if pack(name).is_none() {
                let names: Vec<_> = PACKS.iter().map(|(name, _)| *name).collect();
                return Err(invalid_policy(
                    &format!("extends[{index}]"),
                    format!(
                        "there is no predicate pack named {name:?}; the packs are {}",
                        names.join(" and ")
                    ),
                ));
            }
        }
        self.subjects.validate()?;
        for (name, predicate) in &self.predicates {
            predicate.validate(name)?;
        }
        let fractions = [
            (
                "resolution.trust_margin",
                self.resolution.as_ref().and_then(|r| r.trust_margin),
            ),
            (
                "trust.trusted_threshold",
                self.trust.as_ref().and_then(|t| t.trusted_threshold),
            ),
        ];
        for (path, fraction) in fractions {
            if fraction.is_some_and(|fraction| !(0.0..=1.0).contains(&fraction)) {
                return Err(invalid_policy(path, "must be between 0 and 1"));
            }
        }
        Ok(())
    }
}

impl Subjects {
    fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }

    /// Every name is a subject, and every alias names exactly one subject.
    fn validate(&self) -> Result<()> {
        let subjects: Vec<_> = self.aliases.keys().map(|s| canonical_name(s)).collect();
        let mut aliased: BTreeMap<String, &str> = BTreeMap::new();
        for (subject, aliases) in &self.aliases {
            let path = format!("subjects.aliases.{subject}");
            check_subject(&path, subject)?;
            for (index, alias) in aliases.iter().enumerate() {
                let path = format!("{path}[{index}]");
                check_subject(&path, alias)?;
                let name = canonical_name(alias);
                if subjects.contains(&name) {
                    return Err(invalid_policy(
                        &path,
                        format!("{alias} has aliases of its own, so it can't also be an alias"),
                    ));
                }
                if let Some(other) = aliased.insert(name, subject) {
                    return Err(invalid_policy(
                        &path,
                        format!("{alias} is already an alias of {other}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Predicate {
    fn validate(&self, name: &str) -> Result<()> {
        let path = format!("predicates.{name}");
        let snake_case = name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !snake_case {
            return Err(invalid_policy(
                &path,
                "predicate names are lower_snake_case, such as deployed_version",
            ));
        }
        if let Some(ValueType::Enum(members)) = &self.value_type {
            let path = format!("{path}.value_type.enum");
            if members.is_empty() {
                return Err(invalid_policy(&path, "an enum needs at least one member"));
            }
            for (index, member) in members.iter().enumerate() {
                if members[..index].contains(member) {
                    return Err(invalid_policy(
                        &format!("{path}[{index}]"),
                        format!("{member:?} is listed twice"),
                    ));
                }
            }
        }
        Ok(())
    }
}

// Written by hand so that an enum is the map `{ enum: [...] }` in YAML as well as JSON, where
// serde_yaml would write a derived newtype variant as a tag.
impl Serialize for ValueType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let name = match self {
            ValueType::Text => "text",
            ValueType::Number => "number",
            ValueType::Boolean => "boolean",
            ValueType::Datetime => "datetime",
            ValueType::Version => "version",
            ValueType::Subject => "subject",
            ValueType::Enum(members) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("enum", members)?;
                return map.end();
            }
        };
        serializer.serialize_str(name)
    }
}

impl ValueType {
    /// Whether `value` is a value of this type.
    fn admits(&self, value: &Value) -> bool {
        match self {
            ValueType::Text | ValueType::Version => value.is_string(),
            ValueType::Number => value.is_number(),
            ValueType::Boolean => value.is_boolean(),
            ValueType::Datetime => instant(value).is_some(),
            ValueType::Subject => named_subject(value).is_some_and(is_subject),
            ValueType::Enum(members) => value
                .as_str()
                .is_some_and(|value| members.iter().any(|member| member == value)),
        }
    }

    /// What a value of this type is, for an error message.
    fn describe(&self) -> String {
        match self {
            ValueType::Text => "text".into(),
            ValueType::Number => "a number".into(),
            ValueType::Boolean => "true or false".into(),
            ValueType::Datetime => "an RFC 3339 date and time such as 2026-11-01T09:00:00Z".into(),
            ValueType::Version => "a version such as 2.14.0".into(),
            ValueType::Subject => "a subject such as team:payments".into(),
            ValueType::Enum(members) => match members.split_last() {
                Some((only, [])) => format!("only {only}"),
                Some((last, rest)) => format!("one of {} or {last}", rest.join(", ")),
                None => "no value".into(),
            },
        }
    }
}

impl TryFrom<String> for VerifierName {
    type Error = String;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        let instance = ["http.", "exec."].iter().any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty())
        });
        if instance || BUILT_IN_VERIFIERS.contains(&name.as_str()) {
            return Ok(VerifierName(name));
        }
        Err(format!(
            "there is no verifier named {name:?}; the verifiers are {}, http.<name> and \
             exec.<name>",
            BUILT_IN_VERIFIERS.join(", ")
        ))
    }
}

impl From<VerifierName> for String {
    fn from(name: VerifierName) -> Self {
        name.0
    }
}

impl TryFrom<String> for Interval {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let valid = text
            .strip_suffix(['s', 'm', 'h', 'd'])
            .is_some_and(|count| !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()));
        if valid {
            return Ok(Interval(text));
        }
        Err(format!(
            "{text:?} is not an interval like 45s, 30m, 1h or 90d"
        ))
    }
}

impl From<Interval> for String {
    fn from(interval: Interval) -> Self {
        interval.0
    }
}

impl PolicyVersion {
    /// Parses and validates `text` as a policy document applied to `scope` at `at`.
    pub(crate) fn new(scope: Scope, text: &str, at: OffsetDateTime) -> Result<Self> {
        let document = PolicyDocument::from_yaml(text)?;
        if scope.level != ScopeLevel::Org && !document.subjects.is_empty() {
            return Err(invalid_policy(
                "subjects",
                "aliases are tenant-wide, so a project overlay can't declare them; declare them \
                 in the tenant policy",
            ));
        }
        Ok(PolicyVersion {
            id: format!("pol_{}", ulid::Ulid::from_datetime(at.into())),
            scope,
            packs: packs_extended_by(&document),
            document,
        })
    }
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            versions: Vec::new(),
            packs: Vec::new(),
            subjects: Subjects::default(),
            predicates: BTreeMap::new(),
            resolution: ResolutionPolicy {
                trust_margin: Some(0.25),
                on_unresolved: Some(OnUnresolved::Contested),
            },
            trust: TrustPolicy {
                trusted_threshold: Some(0.80),
            },
            freshness: FreshnessIntervals {
                slow: Some(Interval("90d".into())),
                volatile: Some(Interval("1h".into())),
            },
            subject_of: BTreeMap::new(),
        }
    }
}

impl Policy {
    /// The policy a document sets up when applied to a tenant with no policy yet.
    pub fn from_yaml(text: &str) -> Result<Self> {
        let document = PolicyDocument::from_yaml(text)?;
        let mut policy = Policy::default();
        policy.layer_with_packs(&document, &packs_extended_by(&document));
        Ok(policy)
    }

    /// The policy `versions` set up, each layered over the ones before it.
    pub(crate) fn layered(versions: &[PolicyVersion]) -> Self {
        let mut policy = Policy::default();
        for version in versions {
            policy.versions.push(AppliedVersion {
                id: version.id.clone(),
                scope: version.scope.clone(),
            });
            policy.layer_with_packs(&version.document, &version.packs);
        }
        policy
    }

    /// How many aliases resolve to a subject.
    pub fn alias_count(&self) -> usize {
        self.subject_of.len()
    }

    /// How `name` behaves: as declared, or as an undeclared predicate does.
    pub fn predicate(&self, name: &str) -> Cow<'_, Predicate> {
        self.predicates
            .get(name)
            .map_or_else(|| Cow::Owned(Predicate::default()), Cow::Borrowed)
    }

    /// Whether two values of `predicate` are the same value, compared by its declared value
    /// type. Values of an undeclared type, or that don't fit their declared one, are the same
    /// when they are identical canonical JSON.
    pub fn same_value(&self, predicate: &str, a: &Value, b: &Value) -> bool {
        self.value_type(predicate)
            .and_then(|value_type| self.same_typed_value(value_type, a, b))
            .unwrap_or_else(|| {
                let canonical = |value: &Value| {
                    serde_jcs::to_string(&self.resolve_reference(value.clone())).ok()
                };
                canonical(a) == canonical(b)
            })
    }

    /// A subject as Ledger keeps it: trimmed, lower-cased, and resolved through the aliases.
    pub fn canonical_subject(&self, subject: &str) -> String {
        let name = canonical_name(subject);
        self.subject_of.get(&name).cloned().unwrap_or(name)
    }

    /// Checks a claim against the policy, and returns its subject and value as Ledger keeps
    /// them, with hints for an undeclared predicate. The error names the field that is wrong
    /// and why.
    pub(crate) fn admit(
        &self,
        subject: &str,
        predicate: &str,
        value: Value,
    ) -> Result<Admitted, (&'static str, String)> {
        if !is_subject(subject) {
            return Err(("subject", not_a_subject(subject)));
        }
        Ok(Admitted {
            subject: self.canonical_subject(subject),
            value: self
                .check_value(predicate, value)
                .map_err(|reason| ("value", reason))?,
            hints: self.hint_for(predicate).into_iter().collect(),
        })
    }

    /// Checks a value against its predicate's declared value type, and returns it as Ledger
    /// keeps it, with any subject it names resolved through the aliases. The error says what
    /// the predicate takes.
    fn check_value(&self, predicate: &str, value: Value) -> Result<Value, String> {
        let Some(value_type) = self.value_type(predicate) else {
            return Ok(self.resolve_reference(value));
        };
        if !value_type.admits(&value) {
            return Err(format!(
                "{predicate} takes {}, not {value}",
                value_type.describe()
            ));
        }
        Ok(match (value_type, value) {
            (ValueType::Subject, Value::String(subject)) => {
                Value::String(self.canonical_subject(&subject))
            }
            (_, value) => self.resolve_reference(value),
        })
    }

    /// A hint for a claim on `predicate` when the policy declares predicates but not this one,
    /// naming the declared predicate it most resembles if any is close.
    fn hint_for(&self, predicate: &str) -> Option<String> {
        if self.predicates.is_empty() || self.predicates.contains_key(predicate) {
            return None;
        }
        let nearest = self
            .predicates
            .keys()
            .map(|declared| (strsim::jaro_winkler(predicate, declared), declared))
            .filter(|(similarity, _)| *similarity >= HINT_SIMILARITY)
            .max_by(|a, b| a.0.total_cmp(&b.0));
        Some(match nearest {
            Some((_, declared)) => {
                format!("undeclared predicate '{predicate}'; did you mean '{declared}'?")
            }
            None => format!(
                "undeclared predicate '{predicate}'; it is treated as single-valued and slow \
                 until the policy declares it"
            ),
        })
    }

    /// Whether `a` and `b` are the same value of `value_type`, or None if either doesn't fit it.
    fn same_typed_value(&self, value_type: &ValueType, a: &Value, b: &Value) -> Option<bool> {
        Some(match value_type {
            ValueType::Text => folded(a.as_str()?) == folded(b.as_str()?),
            ValueType::Number => same_number(a.as_number()?, b.as_number()?),
            ValueType::Boolean | ValueType::Enum(_) => a == b,
            ValueType::Datetime => instant(a)? == instant(b)?,
            ValueType::Version => {
                let (a, b) = (a.as_str()?, b.as_str()?);
                match (semver(a), semver(b)) {
                    (Some(a), Some(b)) => a == b,
                    _ => folded(a) == folded(b),
                }
            }
            ValueType::Subject => {
                self.canonical_subject(named_subject(a)?)
                    == self.canonical_subject(named_subject(b)?)
            }
        })
    }

    fn value_type(&self, predicate: &str) -> Option<&ValueType> {
        self.predicates.get(predicate)?.value_type.as_ref()
    }

    /// `value`, with the subject a `{ "type": "subject", "v": … }` reference names resolved.
    fn resolve_reference(&self, mut value: Value) -> Value {
        if value.get("type").and_then(Value::as_str) == Some("subject")
            && let Some(Value::String(subject)) = value.get_mut("v")
        {
            *subject = self.canonical_subject(subject);
        }
        value
    }

    /// Layers the packs a document extends, then the document itself, over this policy.
    fn layer_with_packs(
        &mut self,
        document: &PolicyDocument,
        packs: &BTreeMap<String, PolicyDocument>,
    ) {
        for name in &document.extends {
            if let Some(pack) = packs.get(name) {
                self.layer(pack);
            }
            if !self.packs.contains(name) {
                self.packs.push(name.clone());
            }
        }
        self.layer(document);
    }

    /// Lets a document's declarations replace this policy's: each predicate whole, and each
    /// resolution, trust and freshness field it sets. Its aliases join this policy's.
    fn layer(&mut self, document: &PolicyDocument) {
        self.predicates.extend(document.predicates.clone());
        for (subject, aliases) in &document.subjects.aliases {
            let subject = canonical_name(subject);
            for alias in aliases {
                let alias = canonical_name(alias);
                self.subject_of.insert(alias.clone(), subject.clone());
                self.subjects
                    .aliases
                    .entry(subject.clone())
                    .or_default()
                    .push(alias);
            }
        }
        if let Some(resolution) = &document.resolution {
            let ours = &mut self.resolution;
            ours.trust_margin = resolution.trust_margin.or(ours.trust_margin);
            ours.on_unresolved = resolution.on_unresolved.or(ours.on_unresolved);
        }
        if let Some(trust) = &document.trust {
            let ours = &mut self.trust;
            ours.trusted_threshold = trust.trusted_threshold.or(ours.trusted_threshold);
        }
        if let Some(freshness) = &document.freshness {
            let ours = &mut self.freshness;
            ours.slow = freshness.slow.clone().or(ours.slow.take());
            ours.volatile = freshness.volatile.clone().or(ours.volatile.take());
        }
    }
}

impl PredicatePack {
    /// Every pack this build ships, in the order the contracts list them.
    pub fn all() -> Vec<Self> {
        PACKS
            .iter()
            .map(|(name, _)| PredicatePack {
                name: (*name).to_owned(),
                predicates: pack(name).expect("a shipped pack").predicates,
            })
            .collect()
    }
}

/// The shipped pack named `name`.
fn pack(name: &str) -> Option<PolicyDocument> {
    let (_, text) = PACKS.iter().find(|(pack, _)| *pack == name)?;
    Some(PolicyDocument::from_yaml(text).expect("a shipped pack is a valid policy document"))
}

/// The shipped packs a validated document extends, by name.
fn packs_extended_by(document: &PolicyDocument) -> BTreeMap<String, PolicyDocument> {
    document
        .extends
        .iter()
        .filter_map(|name| Some((name.clone(), pack(name)?)))
        .collect()
}

/// The subject a value names: a string, or the `v` of a `{ "type": "subject", "v": … }`
/// reference.
fn named_subject(value: &Value) -> Option<&str> {
    match value {
        Value::String(subject) => Some(subject),
        Value::Object(reference) if reference.get("type")? == "subject" => {
            reference.get("v")?.as_str()
        }
        _ => None,
    }
}

/// Whether two numbers are numerically equal. Integers compare exactly, since an f64 can't
/// tell large ones apart.
fn same_number(a: &Number, b: &Number) -> bool {
    if let (Some(a), Some(b)) = (a.as_i64(), b.as_i64()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (a.as_u64(), b.as_u64()) {
        return a == b;
    }
    a.as_f64() == b.as_f64()
}

/// Text as it is compared: NFKC-normalised, case-folded, trimmed, and with each run of
/// whitespace one space.
fn folded(text: &str) -> String {
    let normalised: String = text.nfkc().collect();
    let folded = caseless::default_case_fold_str(&normalised);
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What decides whether two versions are equal as semver: major, minor, patch and pre-release.
/// A `v` prefix and a missing patch are tolerated, and build metadata is ignored. None when the
/// text isn't a version that way.
fn semver(text: &str) -> Option<(u64, u64, u64, &str)> {
    let text = text.trim();
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let text = text
        .split_once('+')
        .map_or(text, |(version, _build)| version);
    let (core, pre_release) = match text.split_once('-') {
        Some((_, "")) => return None,
        Some(split) => split,
        None => (text, ""),
    };
    let mut numbers = core.split('.').map(|number| {
        let digits = !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit());
        digits.then(|| number.parse::<u64>().ok()).flatten()
    });
    let major = numbers.next()??;
    let minor = numbers.next()??;
    let patch = numbers.next().unwrap_or(Some(0))?;
    if numbers.next().is_some() {
        return None;
    }
    Some((major, minor, patch, pre_release))
}

/// The instant an RFC 3339 date and time names.
fn instant(value: &Value) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok()
}

/// A subject written the way Ledger keeps it: trimmed and lower-cased.
fn canonical_name(subject: &str) -> String {
    subject.trim().to_lowercase()
}

/// Fails unless `subject` is written `kind:name`.
fn check_subject(path: &str, subject: &str) -> Result<()> {
    if is_subject(subject) {
        return Ok(());
    }
    Err(invalid_policy(path, not_a_subject(subject)))
}

fn not_a_subject(subject: &str) -> String {
    format!("{subject:?} is not a subject like service:billing-api")
}

/// Whether `subject` is written `kind:name`.
fn is_subject(subject: &str) -> bool {
    subject
        .trim()
        .split_once(':')
        .is_some_and(|(kind, name)| !kind.is_empty() && !name.is_empty())
}

pub(crate) fn invalid_policy(path: &str, reason: impl std::fmt::Display) -> Error {
    if path == "." {
        return Error::InvalidPolicy(format!("invalid policy: {reason}; nothing was applied"));
    }
    Error::InvalidPolicy(format!(
        "invalid policy at {path}: {reason}; nothing was applied"
    ))
}
