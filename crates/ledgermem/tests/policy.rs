//! `ledgermem policy apply`, `show` and `packs`, run as an operator would run them.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn ledgermem(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ledgermem"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run ledgermem")
}

/// Runs a command against the database at `db`.
fn ledgermem_on(db: &Path, args: &[&str]) -> Output {
    let mut args = args.to_vec();
    args.extend(["--db", db.to_str().unwrap()]);
    ledgermem(&args)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("JSON on stdout")
}

#[test]
fn an_operator_applies_a_tenant_policy_and_a_project_overlay_and_shows_them() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    let tenant_file = dir.path().join("ledgermem.policy.yaml");
    std::fs::write(
        &tenant_file,
        "version: 1
extends: [software-delivery]
subjects:
  aliases:
    service:billing-api: [service:billing]
",
    )
    .unwrap();
    let overlay_file = dir.path().join("billing-api.policy.yaml");
    std::fs::write(
        &overlay_file,
        "version: 1
predicates:
  rollout_percent: { value_type: number, freshness: volatile }
",
    )
    .unwrap();

    let tenant = ledgermem_on(&db, &["policy", "apply", tenant_file.to_str().unwrap()]);
    let overlay = ledgermem_on(
        &db,
        &[
            "policy",
            "apply",
            overlay_file.to_str().unwrap(),
            "--project",
            "billing-api",
        ],
    );

    assert!(tenant.status.success(), "{}", stderr(&tenant));
    assert!(stdout(&tenant).starts_with("Applied policy pol_"));
    assert!(
        stdout(&tenant).ends_with(" to org: 9 predicates, 1 alias.\n"),
        "{}",
        stdout(&tenant)
    );
    assert!(overlay.status.success(), "{}", stderr(&overlay));
    assert!(
        stdout(&overlay).ends_with(" to project:billing-api: 1 predicate, 0 aliases.\n"),
        "{}",
        stdout(&overlay)
    );

    let shown = ledgermem_on(
        &db,
        &["policy", "show", "--project", "billing-api", "--json"],
    );
    assert!(shown.status.success(), "{}", stderr(&shown));
    let policy = json(&shown);
    let scopes: Vec<_> = policy["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|version| version["scope"].as_str().unwrap())
        .collect();
    assert_eq!(scopes, ["org", "project:billing-api"]);
    assert_eq!(
        policy["predicates"]["rollout_percent"]["value_type"],
        "number"
    );
    assert_eq!(
        policy["predicates"]["deployed_version"]["value_type"],
        "version"
    );

    let tenant_only = ledgermem_on(&db, &["policy", "show"]);
    assert!(tenant_only.status.success(), "{}", stderr(&tenant_only));
    let yaml = stdout(&tenant_only);
    assert!(yaml.contains("packs:\n- software-delivery\n"), "{yaml}");
    assert!(
        yaml.contains("  test_status:\n    cardinality: single\n    value_type:\n      enum:\n"),
        "enum value types show as YAML maps: {yaml}"
    );
    assert!(!yaml.contains("rollout_percent"), "{yaml}");
}

#[test]
fn an_invalid_policy_file_is_refused_with_the_path_named_and_nothing_applied() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    let file = dir.path().join("ledgermem.policy.yaml");
    std::fs::write(
        &file,
        "version: 1
extends: [software-delivery]
predicates:
  deploy_count: { value_type: integer }
",
    )
    .unwrap();

    let applied = ledgermem_on(&db, &["policy", "apply", file.to_str().unwrap()]);

    assert_eq!(applied.status.code(), Some(1));
    assert!(
        stderr(&applied).contains("invalid policy at predicates.deploy_count.value_type"),
        "{}",
        stderr(&applied)
    );
    let shown = ledgermem_on(&db, &["policy", "show", "--json"]);
    assert_eq!(json(&shown)["versions"], serde_json::json!([]));
}

#[test]
fn packs_lists_each_shipped_pack_with_its_predicates() {
    let listed = ledgermem(&["policy", "packs"]);

    assert!(listed.status.success(), "{}", stderr(&listed));
    let lines: Vec<String> = stdout(&listed).lines().map(str::to_owned).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("software-delivery "), "{lines:?}");
    assert!(lines[0].contains("deployed_version"), "{lines:?}");
    assert!(lines[1].starts_with("personal-assistant "), "{lines:?}");
    assert!(lines[1].contains("dietary_preference"), "{lines:?}");

    let packs = json(&ledgermem(&["policy", "packs", "--json"]));
    assert_eq!(packs[1]["name"], "personal-assistant");
    assert_eq!(packs[1]["predicates"]["prefers"]["instruction"], true);
}
