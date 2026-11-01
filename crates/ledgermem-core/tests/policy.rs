//! The policy as the engine consults it: how an undeclared predicate behaves, how subjects resolve
//! through aliases, and when two values are the same value (claims-and-integrity.md, section 1).
//!
//! Nothing in the engine compares values yet; the conflict engine will, through this interface.

use ledgermem_core::{Cardinality, FreshnessClass, Policy};
use serde_json::{Value, json};

fn policy() -> Policy {
    Policy::from_yaml(
        "
        version: 1
        extends: [software-delivery]
        subjects:
          aliases:
            team:payments: [team:pay, team:billing-squad]
        predicates:
          summary: { value_type: text }
          replicas: { value_type: number }
          released_at: { value_type: datetime }
        ",
    )
    .unwrap()
}

fn same(predicate: &str, a: Value, b: Value) -> bool {
    policy().same_value(predicate, &a, &b)
}

#[test]
fn an_undeclared_predicate_is_single_valued_slow_and_unverified() {
    let policy = policy();
    let predicate = policy.predicate("on_call_rota");

    assert_eq!(predicate.cardinality, Cardinality::Single);
    assert_eq!(predicate.freshness, FreshnessClass::Slow);
    assert_eq!(predicate.value_type, None);
    assert_eq!(predicate.verifier, None);
}

#[test]
fn a_declared_predicate_behaves_as_declared() {
    let policy = policy();
    let predicate = policy.predicate("uses_dependency");

    assert_eq!(predicate.cardinality, Cardinality::Multi);
    assert_eq!(predicate.freshness, FreshnessClass::Slow);
}

#[test]
fn subjects_are_trimmed_lower_cased_and_resolved_through_aliases() {
    let policy = policy();

    assert_eq!(policy.canonical_subject(" Team:Pay "), "team:payments");
    assert_eq!(policy.canonical_subject("TEAM:PAYMENTS"), "team:payments");
    assert_eq!(policy.canonical_subject("Team:Platform"), "team:platform");
}

#[test]
fn text_is_the_same_after_normalisation_whitespace_and_case_folding() {
    assert!(same(
        "summary",
        json!("  Payments \t Team "),
        json!("payments team")
    ));
    // NFKC folds full-width letters and the "ﬁ" ligature; case folding takes "ß" to "ss".
    assert!(same("summary", json!("ＡＢＣ ﬁles"), json!("abc files")));
    assert!(same("summary", json!("STRASSE"), json!("straße")));

    assert!(!same(
        "summary",
        json!("payments team"),
        json!("payments teams")
    ));
}

#[test]
fn versions_are_the_same_as_semver_with_a_v_prefix_and_missing_patch_tolerated() {
    assert!(same("deployed_version", json!("v2.14"), json!("2.14.0")));
    assert!(same(
        "deployed_version",
        json!("2.14.0+build.7"),
        json!("2.14.0")
    ));
    assert!(same(
        "deployed_version",
        json!("V2.14-rc.1"),
        json!("2.14.0-rc.1")
    ));

    assert!(!same("deployed_version", json!("2.14.1"), json!("2.14.0")));
    assert!(!same(
        "deployed_version",
        json!("2.14.0-rc.1"),
        json!("2.14.0")
    ));
}

#[test]
fn versions_that_are_not_semver_are_compared_as_text() {
    assert!(same(
        "deployed_version",
        json!("Release-42"),
        json!("release-42")
    ));
    assert!(!same(
        "deployed_version",
        json!("release-42"),
        json!("release-43")
    ));
    assert!(!same("deployed_version", json!("2"), json!("2.0.0")));
}

#[test]
fn datetimes_are_the_same_when_they_name_the_same_instant() {
    assert!(same(
        "released_at",
        json!("2026-11-01T09:00:00Z"),
        json!("2026-11-01T14:30:00+05:30")
    ));
    assert!(!same(
        "released_at",
        json!("2026-11-01T09:00:00Z"),
        json!("2026-11-01T09:00:01Z")
    ));
}

#[test]
fn numbers_are_the_same_when_numerically_equal() {
    assert!(same("replicas", json!(3), json!(3.0)));
    assert!(!same("replicas", json!(3), json!(3.5)));
    // Past 2^53 an f64 can't tell these apart; integers compare exactly.
    assert!(!same(
        "replicas",
        json!(9_007_199_254_740_993_u64),
        json!(9_007_199_254_740_992_u64)
    ));
}

#[test]
fn subjects_are_the_same_after_alias_resolution() {
    assert!(same(
        "owner",
        json!("team:pay"),
        json!("Team:Billing-Squad")
    ));
    assert!(same(
        "owner",
        json!({ "type": "subject", "v": "team:pay" }),
        json!("team:payments")
    ));

    assert!(!same("owner", json!("team:pay"), json!("team:platform")));
}

#[test]
fn enum_members_and_booleans_are_the_same_only_when_identical() {
    assert!(same("test_status", json!("passing"), json!("passing")));
    assert!(!same("test_status", json!("passing"), json!("Passing")));
    assert!(!same("branch_exists", json!(true), json!(false)));
}

#[test]
fn undeclared_values_are_the_same_as_identical_canonical_json() {
    assert!(same(
        "on_call_rota",
        json!({ "every": "week", "starts": 1 }),
        json!({ "starts": 1.0, "every": "week" })
    ));

    assert!(!same("on_call_rota", json!("Weekly"), json!("weekly")));
}
