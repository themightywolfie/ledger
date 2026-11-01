//! The trust scorer: how far a claim is trusted at one moment, and why, and whether it is still
//! fresh (claims-and-integrity.md, sections 4 to 6).

use serde::Serialize;
use time::OffsetDateTime;

use crate::claim::{Claim, Status};
use crate::policy::Policy;

/// How far a claim is trusted, and the components the score is built from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Trust {
    /// From 0 to 1.
    pub score: f64,
    /// What the score is built from, so that whoever reads it can explain it.
    pub components: TrustComponents,
}

/// What a trust score is built from: `max(prior, attestation) + corroboration − contradiction`,
/// kept between 0 and 1, then multiplied by `stale_factor`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TrustComponents {
    /// The starting trust of the claim's source kind.
    pub prior: f64,
    /// The strength of the strongest passing attestation, or 0 when nothing attests the claim.
    pub attestation: f64,
    /// What agreement from independent sources adds.
    pub corroboration: f64,
    /// What open contradicting claims take away.
    pub contradiction: f64,
    /// 0.80 when the claim is stale, otherwise 1.
    pub stale_factor: f64,
}

/// What a read score is multiplied by when the claim is stale.
const STALE_FACTOR: f64 = 0.80;

/// A claim as a read finds it at one moment. None of it is stored: it is worked out again at
/// every read. A claim in an open conflict set is contested, not verified, so it is neither
/// trusted nor injectable.
pub(crate) struct Standing {
    pub trust: Trust,
    /// Verified, and scored at or above the trusted threshold.
    pub trusted: bool,
    /// Longer since it was last attested than its predicate's freshness allows.
    pub stale: bool,
    /// Trusted and fresh, so an adapter may place it in a model's instructions.
    pub injectable: bool,
}

/// How `claim` stands at `now`, under `policy`.
pub(crate) fn standing(claim: &Claim, policy: &Policy, now: OffsetDateTime) -> Standing {
    let stale = is_stale(claim, policy, now);
    let trust = score(claim, stale);
    let trusted = claim.status == Status::Verified && trust.score >= policy.trusted_threshold();
    Standing {
        injectable: trusted && !stale,
        trusted,
        stale,
        trust,
    }
}

/// Whether more time has passed than the predicate's freshness allows since the claim was last
/// attested, or since it was recorded if nothing attests it (section 6).
fn is_stale(claim: &Claim, policy: &Policy, now: OffsetDateTime) -> bool {
    policy
        .reverify_interval(&claim.predicate)
        .is_some_and(|interval| now - claim.last_attested_at() > interval)
}

fn score(claim: &Claim, stale: bool) -> Trust {
    let components = TrustComponents {
        prior: claim.source.kind.prior(),
        attestation: claim
            .evidence
            .iter()
            .filter_map(|evidence| evidence.kind.strength())
            .fold(0.0, f64::max),
        // Counted once the conflict engine relates claims to each other (ticket #09).
        corroboration: 0.0,
        contradiction: 0.0,
        stale_factor: if stale { STALE_FACTOR } else { 1.0 },
    };
    let base = components.prior.max(components.attestation);
    // The LLM judge's lower ceiling arrives with the judge (ticket #08).
    let score = (base + components.corroboration - components.contradiction).clamp(0.0, 1.0)
        * components.stale_factor;
    Trust {
        score: rounded(score),
        components,
    }
}

/// `score` to four decimal places. Every product of the formula's two-decimal defaults fits in
/// four, so this only drops floating-point noise such as 0.7599999999999999.
fn rounded(score: f64) -> f64 {
    (score * 10_000.0).round() / 10_000.0
}
