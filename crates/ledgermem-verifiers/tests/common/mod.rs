//! What the built-in verifier tests share.

use ledgermem_core::{Check, Claim, Observation, Outcome, Verifier};
use serde_json::{Value, json};

/// A claim as the engine would hand it to a verifier.
pub fn claim(subject: &str, predicate: &str, value: Value) -> Claim {
    serde_json::from_value(json!({
        "id": "clm_01JB7Q4R2V8M6N0P3S5T7W9X1Y",
        "scope": "project:billing-api",
        "subject": subject,
        "predicate": predicate,
        "value": value,
        "text": "a claim under test",
        "source": { "kind": "agent_claim", "actor": "agent:coder", "channel": "agent" },
        "status": "proposed",
        "valid_from": "2026-11-01T09:00:00Z",
        "valid_to": null,
        "recorded_at": "2026-11-01T09:00:00Z",
    }))
    .unwrap()
}

/// What `verifier` finds about `claim`, checked with the predicate parameters `params`.
pub async fn verify(verifier: &dyn Verifier, claim: &Claim, params: Value) -> Observation {
    let Value::Object(params) = params else {
        panic!("params must be an object");
    };
    verifier
        .verify(Check {
            claim,
            params: &params,
        })
        .await
}

/// Fails unless `observation` came out as `outcome` with a summary saying `summary`.
#[track_caller]
pub fn assert_observed(observation: &Observation, outcome: Outcome, summary: &str) {
    assert_eq!(observation.outcome, outcome, "{observation:?}");
    assert!(
        observation.summary.contains(summary),
        "the summary says {summary:?}: {observation:?}"
    );
}
