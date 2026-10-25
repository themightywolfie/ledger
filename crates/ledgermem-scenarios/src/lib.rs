//! Ledger's integrity scenario suite.
//!
//! A scenario is a YAML document that names callers, plays a sequence of steps through the engine
//! interface and asserts on what each step returns. Every scenario runs against an engine built
//! with in-memory SQLite and a test clock, so the results are deterministic.
//!
//! ```yaml
//! name: an agent's claim is recalled as proposed
//! clock: 2026-10-25T09:00:00Z
//! callers:
//!   coder: { kind: agent, project: billing-api }
//! steps:
//!   - as: coder
//!     remember: { subject: "service:billing-api", predicate: deployed_version, value: "2.14.0", text: "…" }
//!     expect: { status: proposed }
//!   - advance: 38m
//!   - as: coder
//!     recall: { query: "billing-api version" }
//!     expect: { answer_set: [ { status: proposed, age: 38m } ] }
//! ```
//!
//! An `expect` is matched as a subset of the step's JSON outcome: objects need only the keys the
//! scenario lists, arrays must have the same length, and numbers match within 1e-9.

use std::collections::BTreeMap;
use std::sync::Arc;

use ledgermem_core::{
    Caller, ClaimInput, JournalQuery, Ledger, Permission, Recall, Remember, TestClock,
};
use ledgermem_sqlite::SqliteStore;
use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    name: String,
    #[serde(with = "time::serde::rfc3339")]
    clock: OffsetDateTime,
    callers: BTreeMap<String, CallerSpec>,
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallerSpec {
    kind: CallerKind,
    #[serde(default = "local")]
    tenant: String,
    #[serde(default = "local")]
    user: String,
    project: Option<String>,
    agent: Option<String>,
    permissions: Option<Vec<Permission>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CallerKind {
    Agent,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    #[serde(rename = "as")]
    caller: Option<String>,
    advance: Option<String>,
    remember: Option<ClaimInput>,
    recall: Option<Recall>,
    journal: Option<JournalQuery>,
    expect: Option<Value>,
}

fn local() -> String {
    "local".to_owned()
}

/// Runs one scenario, returning a description of the first failing step.
pub async fn run(yaml: &str) -> Result<(), String> {
    let scenario: Scenario =
        serde_yaml::from_str(yaml).map_err(|e| format!("unreadable scenario: {e}"))?;
    let clock = Arc::new(TestClock::new(scenario.clock));
    let store = SqliteStore::open_in_memory().map_err(|e| e.to_string())?;
    let ledger = Ledger::new(Arc::new(store), clock.clone());

    let mut callers = BTreeMap::new();
    for (name, spec) in &scenario.callers {
        ledger
            .open_tenant(&spec.tenant)
            .await
            .map_err(|e| format!("caller {name}: {e}"))?;
        callers.insert(name.clone(), build_caller(name, spec));
    }

    for (index, step) in scenario.steps.into_iter().enumerate() {
        let at = format!("{:?}, step {}", scenario.name, index + 1);
        run_step(&ledger, &clock, &callers, step)
            .await
            .map_err(|e| format!("{at}: {e}"))?;
    }
    Ok(())
}

fn build_caller(name: &str, spec: &CallerSpec) -> Caller {
    let CallerKind::Agent = spec.kind;
    let agent = spec.agent.as_deref().unwrap_or(name);
    let permissions = spec
        .permissions
        .clone()
        .unwrap_or_else(|| vec![Permission::Read, Permission::Propose, Permission::Attest]);
    let caller = Caller::agent(&spec.tenant, agent)
        .with_user(&spec.user)
        .with_permissions(permissions);
    match &spec.project {
        Some(project) => caller.with_project(project),
        None => caller,
    }
}

async fn run_step(
    ledger: &Ledger,
    clock: &TestClock,
    callers: &BTreeMap<String, Caller>,
    step: Step,
) -> Result<(), String> {
    if let Some(by) = &step.advance {
        clock.advance(parse_duration(by)?);
        return Ok(());
    }

    let name = step
        .caller
        .as_deref()
        .ok_or("a step needs `as` or `advance`")?;
    let caller = callers
        .get(name)
        .ok_or_else(|| format!("unknown caller {name:?}"))?;

    let outcome = match (step.remember, step.recall, step.journal) {
        (Some(claim), None, None) => {
            let outcome = ledger
                .remember(
                    caller,
                    Remember {
                        claims: vec![claim],
                    },
                )
                .await;
            to_json(outcome)?
                .pointer("/results/0")
                .cloned()
                .unwrap_or(Value::Null)
        }
        (None, Some(recall), None) => to_json(ledger.recall(caller, recall).await)?,
        (None, None, Some(query)) => to_json(ledger.journal(caller, query).await)?,
        _ => return Err("a step needs exactly one of remember, recall or journal".into()),
    };

    match step.expect {
        Some(expected) => matches(&expected, &outcome, "outcome"),
        None => Ok(()),
    }
}

/// Parses a duration such as `45s`, `38m`, `3h` or `90d`.
fn parse_duration(text: &str) -> Result<time::Duration, String> {
    let invalid = || format!("advance: {text:?} is not a duration like 45s, 38m, 3h or 90d");
    let split = text.len().checked_sub(1).ok_or_else(invalid)?;
    let (count, unit) = text.split_at(split);
    let count: i64 = count.parse().map_err(|_| invalid())?;
    match unit {
        "s" => Ok(time::Duration::seconds(count)),
        "m" => Ok(time::Duration::minutes(count)),
        "h" => Ok(time::Duration::hours(count)),
        "d" => Ok(time::Duration::days(count)),
        _ => Err(invalid()),
    }
}

fn to_json<T: serde::Serialize>(outcome: ledgermem_core::Result<T>) -> Result<Value, String> {
    let outcome = outcome.map_err(|e| format!("engine error: {e}"))?;
    serde_json::to_value(outcome).map_err(|e| e.to_string())
}

fn matches(expected: &Value, actual: &Value, path: &str) -> Result<(), String> {
    match (expected, actual) {
        (Value::Object(want), Value::Object(got)) => want.iter().try_for_each(|(key, want)| {
            let path = format!("{path}.{key}");
            let got = got.get(key).ok_or_else(|| format!("{path} is missing"))?;
            matches(want, got, &path)
        }),
        (Value::Array(want), Value::Array(got)) => {
            if want.len() != got.len() {
                return Err(format!(
                    "{path} has {} items, expected {}: {actual}",
                    got.len(),
                    want.len()
                ));
            }
            want.iter()
                .zip(got)
                .enumerate()
                .try_for_each(|(i, (want, got))| matches(want, got, &format!("{path}[{i}]")))
        }
        (Value::Number(want), Value::Number(got)) => {
            let (want, got) = (want.as_f64(), got.as_f64());
            match (want, got) {
                (Some(w), Some(g)) if (w - g).abs() < 1e-9 => Ok(()),
                _ => Err(format!("{path} is {got:?}, expected {want:?}")),
            }
        }
        _ if expected == actual => Ok(()),
        _ => Err(format!("{path} is {actual}, expected {expected}")),
    }
}
