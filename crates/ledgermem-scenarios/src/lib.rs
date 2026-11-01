//! Ledger's integrity scenario suite.
//!
//! A scenario is a YAML document that names callers, plays a sequence of steps through the engine
//! interface and asserts on what each step returns. Every scenario runs against an engine built
//! with in-memory SQLite and a test clock, so the results are deterministic.
//!
//! ```yaml
//! name: an agent's claim is recalled as proposed
//! clock: 2026-10-25T09:00:00Z
//! policy: { version: 1, extends: [software-delivery] }
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
//! The optional `policy` is applied to every tenant the callers use before the first step.
//!
//! The optional `verifiers` are scripted verifiers, registered under the names policies use, each
//! answering by claim subject. A `script` step changes their answers mid-scenario:
//!
//! ```yaml
//! verifiers:
//!   test_run:
//!     "repo:acme/billing-api": { outcome: fail, observed: failing, summary: "3 of 412 tests fail" }
//! steps:
//!   - script: { test_run: { "repo:acme/billing-api": { outcome: pass, observed: passing } } }
//! ```
//!
//! Claim ids are generated, so a remember step can `label` the claim it writes, and an `attest`
//! step can name it by that label as `claim`, in place of a `claim_id`.
//!
//! An `expect` is matched as a subset of the step's JSON outcome: objects need only the keys the
//! scenario lists, arrays must have the same length, and numbers match within 1e-9.

use std::collections::BTreeMap;
use std::sync::Arc;

use ledgermem_core::{
    ApplyPolicy, Attest, Caller, Channel, ClaimInput, JournalQuery, Ledger, Observation, Outcome,
    Permission, PolicyQuery, Recall, Remember, ScriptedVerifier, TestClock, Verifier, VerifierName,
    Verifiers,
};
use ledgermem_sqlite::SqliteStore;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    name: String,
    #[serde(with = "time::serde::rfc3339")]
    clock: OffsetDateTime,
    /// A policy document applied to each tenant before the first step.
    policy: Option<serde_yaml::Value>,
    callers: BTreeMap<String, CallerSpec>,
    /// Scripted verifiers, and what each answers at first.
    #[serde(default)]
    verifiers: Script,
    steps: Vec<Step>,
}

/// What scripted verifiers answer: for each verifier, an observation per claim subject.
type Script = BTreeMap<VerifierName, BTreeMap<String, ScriptedObservation>>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptedObservation {
    outcome: Outcome,
    #[serde(default)]
    observed: Value,
    /// Defaults to `scripted`.
    summary: Option<String>,
}

/// The scenario's scripted verifiers, by name.
type Scripted = BTreeMap<VerifierName, Arc<ScriptedVerifier>>;

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
    /// The channel an operator works through: `cli`, the default, or `visualizer`.
    channel: Option<Channel>,
    permissions: Option<Vec<Permission>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CallerKind {
    Agent,
    /// A framework integration, which may declare where its claims came from.
    Integration,
    /// A person looking after the tenant from the command line.
    Operator,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    #[serde(rename = "as")]
    caller: Option<String>,
    advance: Option<String>,
    /// Changes what scripted verifiers answer from now on.
    script: Option<Script>,
    remember: Option<ClaimInput>,
    /// A name for the claim a remember step writes, which later attest steps can use.
    label: Option<String>,
    /// An attest request, naming the claim by `claim_id` or by the `claim` label it was given.
    attest: Option<Value>,
    recall: Option<Recall>,
    journal: Option<JournalQuery>,
    /// Verify every journal in the caller's tenant. Takes no arguments: `verify: {}`.
    verify: Option<Value>,
    /// Rebuild the caller's tenant's projections from its journals: `reindex: {}`.
    reindex: Option<Value>,
    apply_policy: Option<PolicyStep>,
    show_policy: Option<PolicyQuery>,
    expect: Option<Value>,
}

/// A policy document to apply, written in the scenario as YAML, and the project it overlays.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyStep {
    project: Option<String>,
    /// A YAML mapping, or a string holding the document's text verbatim.
    document: serde_yaml::Value,
}

impl PolicyStep {
    fn into_request(self) -> Result<ApplyPolicy, String> {
        Ok(ApplyPolicy {
            document: document_text(self.document)?,
            project: self.project,
        })
    }
}

/// The text of a policy document written in a scenario.
fn document_text(document: serde_yaml::Value) -> Result<String, String> {
    match document {
        serde_yaml::Value::String(text) => Ok(text),
        document => serde_yaml::to_string(&document).map_err(|e| e.to_string()),
    }
}

/// The one engine operation a step plays.
enum Action {
    Remember(ClaimInput),
    Attest(Value),
    Recall(Recall),
    Journal(JournalQuery),
    Verify,
    Reindex,
    ApplyPolicy(ApplyPolicy),
    ShowPolicy(PolicyQuery),
}

impl Step {
    fn take_action(&mut self) -> Result<Action, String> {
        let actions = [
            self.remember.take().map(Action::Remember),
            self.attest.take().map(Action::Attest),
            self.recall.take().map(Action::Recall),
            self.journal.take().map(Action::Journal),
            self.verify.take().map(|_| Action::Verify),
            self.reindex.take().map(|_| Action::Reindex),
            match self.apply_policy.take() {
                Some(step) => Some(Action::ApplyPolicy(step.into_request()?)),
                None => None,
            },
            self.show_policy.take().map(Action::ShowPolicy),
        ];
        let mut present = actions.into_iter().flatten();
        match (present.next(), present.next()) {
            (Some(action), None) => Ok(action),
            _ => Err(
                "a step needs exactly one of remember, attest, recall, journal, verify, \
                      reindex, apply_policy or show_policy"
                    .into(),
            ),
        }
    }
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
    let scripted: Scripted = scenario
        .verifiers
        .keys()
        .map(|name| (name.clone(), Arc::default()))
        .collect();
    let verifiers = scripted
        .iter()
        .fold(Verifiers::new(), |verifiers, (name, verifier)| {
            verifiers.with(name.clone(), verifier.clone() as Arc<dyn Verifier>)
        });
    apply_script(&scripted, scenario.verifiers)?;
    let ledger = Ledger::new(Arc::new(store), clock.clone()).with_verifiers(verifiers);

    let mut callers = BTreeMap::new();
    for (name, spec) in &scenario.callers {
        ledger
            .open_tenant(&spec.tenant)
            .await
            .map_err(|e| format!("caller {name}: {e}"))?;
        callers.insert(
            name.clone(),
            build_caller(name, spec).map_err(|e| format!("caller {name}: {e}"))?,
        );
    }
    if let Some(document) = scenario.policy {
        let document = document_text(document)?;
        let tenants: std::collections::BTreeSet<_> =
            scenario.callers.values().map(|spec| &spec.tenant).collect();
        for tenant in tenants {
            let operator = Caller::operator(tenant, "scenario", Channel::Cli)
                .with_permissions([Permission::Operator]);
            let apply = ApplyPolicy {
                document: document.clone(),
                project: None,
            };
            ledger
                .apply_policy(&operator, apply)
                .await
                .map_err(|e| format!("policy: {e}"))?;
        }
    }

    let world = World {
        ledger,
        clock,
        scripted,
        callers,
    };
    let mut labels = Labels::new();
    for (index, step) in scenario.steps.into_iter().enumerate() {
        let at = format!("{:?}, step {}", scenario.name, index + 1);
        run_step(&world, &mut labels, step)
            .await
            .map_err(|e| format!("{at}: {e}"))?;
    }
    Ok(())
}

/// What a scenario's steps act on.
struct World {
    ledger: Ledger,
    clock: Arc<TestClock>,
    scripted: Scripted,
    callers: BTreeMap<String, Caller>,
}

/// The ids of the claims remember steps labelled, by label.
type Labels = BTreeMap<String, Value>;

fn build_caller(name: &str, spec: &CallerSpec) -> Result<Caller, String> {
    use Permission::{Admin, Attest, DeclareSource, Operator, Propose, Read};
    let (caller, default_permissions) = match spec.kind {
        CallerKind::Operator => {
            let permissions = spec
                .permissions
                .clone()
                .unwrap_or_else(|| vec![Read, Operator, Admin]);
            let channel = match spec.channel {
                None | Some(Channel::Cli) => Channel::Cli,
                Some(Channel::Visualizer) => Channel::Visualizer,
                Some(_) => return Err("an operator works through cli or visualizer".into()),
            };
            return Ok(Caller::operator(&spec.tenant, name, channel).with_permissions(permissions));
        }
        _ if spec.channel.is_some() => {
            return Err(
                "only an operator picks its channel; an agent writes through agent, \
                        and an integration through integration"
                    .into(),
            );
        }
        CallerKind::Agent => (
            Caller::agent(&spec.tenant, spec.agent.as_deref().unwrap_or(name)),
            vec![Read, Propose, Attest],
        ),
        CallerKind::Integration => (
            Caller::integration(&spec.tenant, name),
            vec![Read, Propose, Attest, DeclareSource],
        ),
    };
    let caller = caller
        .with_user(&spec.user)
        .with_permissions(spec.permissions.clone().unwrap_or(default_permissions));
    Ok(match &spec.project {
        Some(project) => caller.with_project(project),
        None => caller,
    })
}

/// Makes each scripted verifier answer as `script` says from now on.
fn apply_script(scripted: &Scripted, script: Script) -> Result<(), String> {
    for (name, subjects) in script {
        let verifier = scripted.get(&name).ok_or_else(|| {
            format!("script: {name} isn't a scripted verifier; declare it under `verifiers`")
        })?;
        for (subject, observation) in subjects {
            let summary = observation.summary.unwrap_or_else(|| "scripted".to_owned());
            verifier.script(
                &subject,
                Observation::new(observation.outcome, observation.observed, summary),
            );
        }
    }
    Ok(())
}

async fn run_step(world: &World, labels: &mut Labels, mut step: Step) -> Result<(), String> {
    let World {
        ledger,
        clock,
        scripted,
        callers,
    } = world;
    if let Some(by) = &step.advance {
        clock.advance(parse_duration(by)?);
        return Ok(());
    }
    if let Some(script) = step.script.take() {
        return apply_script(scripted, script);
    }

    let name = step
        .caller
        .as_deref()
        .ok_or("a step needs `as`, `advance` or `script`")?;
    let caller = callers
        .get(name)
        .ok_or_else(|| format!("unknown caller {name:?}"))?;

    let outcome = match step.take_action()? {
        Action::Remember(claim) => {
            let remember = Remember {
                claims: vec![claim],
            };
            let outcome = to_json(ledger.remember(caller, remember).await).map(|outcome| {
                outcome
                    .pointer("/results/0")
                    .cloned()
                    .unwrap_or(Value::Null)
            });
            if let (Some(label), Ok(outcome)) = (step.label.take(), &outcome) {
                labels.insert(label, outcome["claim_id"].clone());
            }
            outcome
        }
        Action::Attest(request) => {
            let attest = attest_request(request, labels)?;
            to_json(ledger.attest(caller, attest).await)
        }
        Action::Recall(recall) => to_json(ledger.recall(caller, recall).await),
        Action::Journal(query) => to_json(ledger.journal(caller, query).await),
        Action::Verify => to_json(ledger.verify_journals(caller).await),
        Action::Reindex => to_json(ledger.reindex(caller).await),
        Action::ApplyPolicy(apply) => to_json(ledger.apply_policy(caller, apply).await),
        Action::ShowPolicy(query) => to_json(ledger.policy(caller, query).await),
    };

    let expected = step.expect.unwrap_or(Value::Null);
    let expects_error = expected.get("error").is_some();
    match outcome {
        Ok(outcome) if expects_error => Err(format!("expected an error, got {outcome}")),
        Ok(_) if expected.is_null() => Ok(()),
        Ok(outcome) => matches(&expected, &outcome, "outcome"),
        Err(error) if expects_error => matches(&expected, &error, "outcome"),
        Err(error) => Err(format!("engine error: {error}")),
    }
}

/// An attest step's request, with the claim it names by label given by id.
fn attest_request(mut request: Value, labels: &Labels) -> Result<Attest, String> {
    if let Some(fields) = request.as_object_mut()
        && let Some(label) = fields.remove("claim")
    {
        let id = label
            .as_str()
            .and_then(|label| labels.get(label))
            .ok_or_else(|| format!("attest: no remember step is labelled {label}"))?;
        fields.insert("claim_id".into(), id.clone());
    }
    serde_json::from_value(request).map_err(|e| format!("attest: {e}"))
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

/// The step's outcome as JSON, or its error as `{ "error": { "code", "message" } }`, the body
/// every surface returns.
fn to_json<T: serde::Serialize>(outcome: ledgermem_core::Result<T>) -> Result<Value, Value> {
    let error =
        |code: &str, message: String| json!({ "error": { "code": code, "message": message } });
    match outcome {
        Ok(outcome) => serde_json::to_value(outcome).map_err(|e| error("internal", e.to_string())),
        Err(e) => Err(error(e.code(), e.to_string())),
    }
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
