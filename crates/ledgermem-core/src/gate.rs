//! The write gate's classification: where a claim came from, what backs it at once, and whether it
//! is let in as memory (claims-and-integrity.md, sections 2 and 3). Also the intake of evidence a
//! caller attaches.

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::caller::{Caller, Channel, Permission};
use crate::claim::{Evidence, EvidenceKind, SourceKind, Status};
use crate::journal;
use crate::ops::{ClaimInput, EvidenceInput, EvidenceInputKind, RelayedUserStatement};
use crate::policy::{Admitted, Authority, Policy, Predicate, ValueType, folded};

/// What the write gate decided about one claim.
pub(crate) struct Classification {
    pub kind: SourceKind,
    pub evidence: Vec<EvidenceKind>,
    pub status: Status,
    /// Why the gate treated the claim as it did, where the caller might have expected otherwise.
    pub hints: Vec<String>,
}

/// The directive rules: phrasings that read as an instruction to an agent rather than as a claim
/// about the world (claims-and-integrity.md, section 3). Each is matched against every sentence
/// of a claim's text, and of its value when that is free text, folded to lower case, with `^` at
/// the start of the sentence. The first rule that matches names what the claim reads as.
const DIRECTIVE_RULES: [(&str, &str); 10] = [
    (
        r"\b(always|only) (use|route|send|call)\b",
        "tool routing (\"always use …\")",
    ),
    (
        r"^(use|route|send|call) .*\b(for|to|through|via|instead)\b",
        "tool routing (\"use … for …\")",
    ),
    (
        r"\buser is (a |an |the )?(vip|admin|administrator|superuser|root)\b",
        "a role claim (\"the user is an admin\")",
    ),
    (
        r"\buser has (admin|administrator|root|superuser) (access|rights|privileges|permissions)\b",
        "a role claim (\"the user has admin rights\")",
    ),
    (
        r"\btreat (the |this )?user as\b",
        "a role claim (\"treat the user as …\")",
    ),
    (
        r"\b(ignore|disregard|forget) (all |any )?(of )?(the |your )?(previous|prior|earlier|above|preceding)\b",
        "an instruction to an agent (\"ignore previous …\")",
    ),
    (
        r"\bfrom now on\b",
        "an instruction to an agent (\"from now on …\")",
    ),
    (
        r"\byou (must|should|need to|have to)\b",
        "an instruction to an agent (\"you must …\")",
    ),
    (
        r"^(always|never) \w",
        "an instruction to an agent (\"always …\", \"never …\")",
    ),
    (
        r"^(do not|don['’]t|make sure|be sure to|remember to) \w",
        "an instruction to an agent (\"do not …\")",
    ),
];

static DIRECTIVES: LazyLock<Vec<(Regex, &str)>> = LazyLock::new(|| {
    DIRECTIVE_RULES
        .iter()
        .map(|(pattern, reads_as)| (Regex::new(pattern).expect("a valid rule"), *reads_as))
        .collect()
});

/// Where one sentence ends and the next begins: a line break, or closing punctuation before a
/// space or the end. A full stop inside a version such as 2.14.0 doesn't end one.
static SENTENCE_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[.!?;:]+(\s|$)|\n").expect("a valid pattern"));

/// What an integration with declare_source may say about where a claim came from.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredSource {
    kind: SourceKind,
}

/// Classifies one claim, already admitted by the policy, as the gate lets it in. The error names
/// the field that is wrong and why.
pub(crate) fn classify(
    caller: &Caller,
    input: &ClaimInput,
    admitted: &Admitted,
    policy: &Policy,
) -> Result<Classification, (&'static str, String)> {
    if input
        .relayed_user_statement
        .as_ref()
        .is_some_and(|relayed| relayed.quote.trim().is_empty())
    {
        return Err((
            "relayed_user_statement.quote",
            "give the user's exact words, not an empty quote".into(),
        ));
    }
    let predicate = policy.predicate(&input.predicate);
    let (channel_kind, channel_evidence) = channel_source(caller.channel);
    let mut hints = Vec::new();
    let kind = source_kind(caller, channel_kind, input, &mut hints)?;
    if let Some(reason) = overreach(kind, &input.predicate, &predicate) {
        return Ok(quarantined(kind, reason, hints));
    }

    // A person writing for themselves is their own evidence, unless they report the claim came
    // from somewhere less trusted.
    let mut evidence: Vec<_> = channel_evidence
        .filter(|_| kind == channel_kind)
        .into_iter()
        .collect();
    if let Some(relayed) = &input.relayed_user_statement {
        match relay(caller, relayed, input, admitted, &predicate, policy) {
            Ok(statement) => evidence.push(statement),
            Err(why) => hints.push(format!(
                "relayed_user_statement doesn't count: {why}, so this is the agent's claim, not \
                 the user's"
            )),
        }
    }
    if !matches!(kind, SourceKind::UserStated | SourceKind::System)
        && let Some(reason) = directive(&input.text, &admitted.value, &predicate)
    {
        let relayed = evidence
            .iter()
            .any(|kind| matches!(kind, EvidenceKind::RelayedUserStatement { .. }));
        if relayed {
            hints.push(
                "to relay what the user prefers, write it as a statement about them, such as \
                 \"the user prefers …\""
                    .into(),
            );
        }
        return Ok(quarantined(kind, reason, hints));
    }
    Ok(Classification {
        kind,
        status: if evidence.is_empty() {
            Status::Proposed
        } else {
            Status::Verified
        },
        evidence,
        hints,
    })
}

/// The source kind a channel gives a claim, and the evidence that writing through it is in
/// itself: a person's own word on the channels a person uses (section 2).
fn channel_source(channel: Channel) -> (SourceKind, Option<EvidenceKind>) {
    match channel {
        Channel::Visualizer | Channel::Cli => (
            SourceKind::UserStated,
            Some(EvidenceKind::HumanConfirmation),
        ),
        // The user's answer becomes a user statement once elicitation lands (ticket #18).
        Channel::Elicitation => (SourceKind::UserStated, None),
        Channel::Agent | Channel::Integration => (SourceKind::AgentClaim, None),
        // Ledger's own re-verification attests claims; it never writes one.
        Channel::Internal => (SourceKind::System, None),
    }
}

/// The claim's source kind: the channel's, or the one an integration with declare_source
/// declares, lowered by any reported source (ADR-0009). Anyone else's `source` is ignored,
/// whatever it holds.
fn source_kind(
    caller: &Caller,
    channel_kind: SourceKind,
    input: &ClaimInput,
    hints: &mut Vec<String>,
) -> Result<SourceKind, (&'static str, String)> {
    let may_declare = caller.channel == Channel::Integration
        && caller.permissions.contains(&Permission::DeclareSource);
    let kind = match &input.source {
        Some(source) if may_declare => declared_kind(source)?,
        Some(_) => {
            hints.push(format!(
                "source is ignored: a claim's source kind comes from the channel and key that \
                 wrote it, so this claim is recorded as {channel_kind}"
            ));
            channel_kind
        }
        None => channel_kind,
    };
    // A reported source only ever lowers trust.
    Ok(match input.reported_source {
        Some(reported) => kind.max(reported.into()),
        None => kind,
    })
}

fn declared_kind(source: &Value) -> Result<SourceKind, (&'static str, String)> {
    match DeclaredSource::deserialize(source) {
        Ok(DeclaredSource {
            kind: SourceKind::System,
        }) => Err((
            "source.kind",
            "an integration can't declare system, which only Ledger's own re-verification \
             writes; declare user_stated, tool_result, document, web, agent_claim or other_agent"
                .into(),
        )),
        Ok(declared) => Ok(declared.kind),
        Err(error) => Err(("source", error.to_string())),
    }
}

/// Why a low-trust source may not set `name` at all, if it may not: the predicate is the user's
/// to state, or it tells agents what to do.
fn overreach(kind: SourceKind, name: &str, predicate: &Predicate) -> Option<String> {
    if !matches!(
        kind,
        SourceKind::Web | SourceKind::Document | SourceKind::OtherAgent
    ) {
        return None;
    }
    if predicate.authority == Some(Authority::User) {
        Some(format!(
            "{kind} sources can't set {name}, which is the user's to state"
        ))
    } else if predicate.instruction {
        Some(format!(
            "{kind} sources can't set {name}, which tells agents what to do"
        ))
    } else {
        None
    }
}

/// The evidence a relayed user statement adds, or why it doesn't count. It counts only from a
/// caller with relay_user, about something the user has authority over, and about the user the
/// caller is bound to: the user's word settles only their own preferences and identity.
fn relay(
    caller: &Caller,
    relayed: &RelayedUserStatement,
    input: &ClaimInput,
    admitted: &Admitted,
    predicate: &Predicate,
    policy: &Policy,
) -> Result<EvidenceKind, String> {
    if !caller.permissions.contains(&Permission::RelayUser) {
        return Err(format!("{} lacks the relay_user permission", caller.actor));
    }
    if predicate.authority != Some(Authority::User) {
        return Err(format!(
            "the user has no authority over {}",
            input.predicate
        ));
    }
    let Some(user) = &caller.bounds.user else {
        return Err(format!(
            "{} isn't bound to a user, so there is no user whose words it relays",
            caller.actor
        ));
    };
    let own_subject = policy.canonical_subject(&format!("user:{user}"));
    if admitted.subject != own_subject {
        return Err(format!(
            "the claim is about {}, and {} relays only for {own_subject}",
            admitted.subject, caller.actor
        ));
    }
    Ok(EvidenceKind::RelayedUserStatement {
        quote: relayed.quote.clone(),
    })
}

/// What a claim's text, or its free-text value, reads as if a directive rule matches it.
fn directive(text: &str, value: &Value, predicate: &Predicate) -> Option<String> {
    let value = match (value, &predicate.value_type) {
        (Value::String(value), None | Some(ValueType::Text)) => Some(value.as_str()),
        _ => None,
    };
    [("text", Some(text)), ("value", value)]
        .into_iter()
        .find_map(|(field, written)| {
            let reads_as = reads_as_directive(written?)?;
            Some(format!(
                "the {field} reads as {reads_as}, and only a claim the user states directly can \
                 instruct agents"
            ))
        })
}

/// The first directive rule `written` matches, sentence by sentence.
fn reads_as_directive(written: &str) -> Option<&'static str> {
    let sentences: Vec<_> = SENTENCE_END
        .split(written)
        .map(|sentence| {
            folded(sentence)
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .to_owned()
        })
        .filter(|sentence| !sentence.is_empty())
        .collect();
    DIRECTIVES.iter().find_map(|(rule, reads_as)| {
        sentences
            .iter()
            .any(|sentence| rule.is_match(sentence))
            .then_some(*reads_as)
    })
}

/// What a caller observed through its tools, as evidence. The error names the piece and field
/// that are wrong and why, as `evidence[1].tool: …`.
pub(crate) fn attached_evidence(
    inputs: &[EvidenceInput],
    now: OffsetDateTime,
) -> Result<Vec<Evidence>, String> {
    inputs
        .iter()
        .enumerate()
        .map(|(index, input)| {
            caller_evidence(input, now)
                .map_err(|(field, reason)| format!("evidence[{index}].{field}: {reason}"))
        })
        .collect()
}

/// What a caller observed through one of its tools, as evidence: hashed, and timestamped when it
/// was observed. It attests nothing until a verifier checks the claim. The error names the field
/// that is wrong and why.
fn caller_evidence(
    input: &EvidenceInput,
    now: OffsetDateTime,
) -> Result<Evidence, (&'static str, String)> {
    if input.tool.trim().is_empty() {
        return Err(("tool", "name the tool you observed it through".into()));
    }
    let observed_at = input.observed_at.unwrap_or(now);
    if observed_at > now {
        return Err(("observed_at", "can't be later than now".into()));
    }
    let EvidenceInputKind::ToolResult = input.kind;
    let hash = journal::content_hash(&json!({
        "tool": input.tool, "ref": input.reference, "excerpt": input.excerpt
    }));
    Ok(Evidence::observed(
        EvidenceKind::ToolResult {
            tool: input.tool.clone(),
            reference: input.reference.clone(),
            excerpt: input.excerpt.clone(),
            hash,
        },
        observed_at,
    ))
}

fn quarantined(kind: SourceKind, reason: String, mut hints: Vec<String>) -> Classification {
    hints.push(format!(
        "quarantined: {reason}; it isn't served as memory, and only an operator can release it"
    ));
    Classification {
        kind,
        evidence: Vec::new(),
        status: Status::Quarantined,
        hints,
    }
}
