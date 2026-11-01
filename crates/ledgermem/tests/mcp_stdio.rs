//! The MCP stdio surface, driven by a real MCP client against the `ledgermem` binary.

use std::path::Path;
use std::process::Stdio;

use rmcp::model::{CallToolRequestParams, ProtocolVersion};
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientLifecycleMode, ClientServiceExt, RoleClient};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, ()>;

async fn connect(db: &Path, args: &[&str]) -> Client {
    connect_with(mcp_command(db, args)).await
}

fn mcp_command(db: &Path, args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ledgermem"));
    command.arg("mcp").arg("--db").arg(db).args(args);
    command
}

async fn connect_with(command: tokio::process::Command) -> Client {
    let transport = TokioChildProcess::new(command).expect("spawn ledgermem");
    ().serve_with_lifecycle(
        transport,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await
    .expect("connect to ledgermem")
}

async fn call(client: &Client, tool: &'static str, arguments: Value) -> Value {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .expect("tool call");
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    result.structured_content.expect("structured content")
}

/// Applies `document` as the tenant policy through `ledgermem policy apply`.
fn apply_policy(db: &Path, document: &str) {
    let policy = db.with_file_name("ledgermem.policy.yaml");
    std::fs::write(&policy, document).unwrap();
    let applied = std::process::Command::new(env!("CARGO_BIN_EXE_ledgermem"))
        .args(["policy", "apply"])
        .arg(&policy)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::null())
        .output()
        .expect("run ledgermem");
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
}

fn deployed_version_claim() -> Value {
    json!({ "claims": [ {
        "subject": "service:billing-api",
        "predicate": "deployed_version",
        "value": "2.14.0",
        "text": "billing-api is running 2.14.0 in production"
    } ] })
}

fn is_age_in_seconds(age: &str) -> bool {
    age.strip_suffix('s')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

#[tokio::test]
async fn lists_remember_and_recall_and_recalls_a_remembered_claim_as_proposed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("data").join("ledgermem.db");
    let client = connect(&db, &["--project", "billing-api", "--agent", "claude-code"]).await;

    let mut tools: Vec<_> = client
        .list_all_tools()
        .await
        .expect("tools/list")
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    tools.sort();
    assert_eq!(tools, ["memory_attest", "memory_recall", "memory_remember"]);

    let remembered = call(&client, "memory_remember", deployed_version_claim()).await;
    let result = &remembered["results"][0];
    assert_eq!(result["status"], "proposed");
    assert_eq!(result["trust"]["score"], 0.30);
    let claim_id = result["claim_id"].as_str().expect("claim id");
    assert!(claim_id.starts_with("clm_"), "{claim_id}");

    let recalled = call(
        &client,
        "memory_recall",
        json!({ "query": "what version of billing-api is deployed?" }),
    )
    .await;
    assert_eq!(recalled["intent"], "current");
    let answer = &recalled["answer_set"][0];
    assert_eq!(answer["claim_id"], claim_id);
    assert_eq!(answer["status"], "proposed");
    assert_eq!(answer["trust"]["score"], 0.30);
    assert_eq!(answer["scope"], "project:billing-api");
    let age = answer["age"].as_str().expect("age");
    assert!(is_age_in_seconds(age), "{age}");

    client.cancel().await.expect("close client");
    assert!(db.is_file(), "no database at {}", db.display());
}

#[tokio::test]
async fn remember_offers_models_relayed_and_reported_sources_but_never_a_source_kind() {
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&dir.path().join("ledgermem.db"), &[]).await;

    let tools = client.list_all_tools().await.expect("tools/list");
    client.cancel().await.expect("close client");

    let remember = tools
        .iter()
        .find(|tool| tool.name == "memory_remember")
        .expect("memory_remember");
    let schema = serde_json::to_string(&remember.input_schema).unwrap();
    assert!(schema.contains("\"relayed_user_statement\""), "{schema}");
    assert!(schema.contains("\"reported_source\""), "{schema}");
    assert!(!schema.contains("\"source\""), "{schema}");
}

#[tokio::test]
async fn a_high_stakes_recall_abstains_from_an_answer_nothing_backs() {
    let dir = tempfile::tempdir().unwrap();
    let client = connect(
        &dir.path().join("ledgermem.db"),
        &["--project", "billing-api"],
    )
    .await;

    let tools = client.list_all_tools().await.expect("tools/list");
    let recall = tools
        .iter()
        .find(|tool| tool.name == "memory_recall")
        .expect("memory_recall");
    let schema = serde_json::to_string(&recall.input_schema).unwrap();
    assert!(schema.contains("\"high_stakes\""), "{schema}");

    call(&client, "memory_remember", deployed_version_claim()).await;
    let query = "what version of billing-api is deployed?";
    let recalled = call(&client, "memory_recall", json!({ "query": query })).await;
    assert_eq!(recalled["guidance"]["code"], "unverified");
    let answer = &recalled["answer_set"][0];
    assert_eq!(answer["trusted"], false);
    assert_eq!(answer["stale"], false);
    assert_eq!(answer["injectable"], false);
    assert_eq!(
        answer["trust"],
        json!({ "score": 0.30, "components": {
            "prior": 0.30, "attestation": 0.0, "corroboration": 0.0, "contradiction": 0.0,
            "stale_factor": 1.0
        } })
    );

    let recalled = call(
        &client,
        "memory_recall",
        json!({ "query": query, "high_stakes": true }),
    )
    .await;
    client.cancel().await.expect("close client");
    assert_eq!(recalled["guidance"]["code"], "unresolved");
    assert_eq!(recalled["answer_set"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn claims_outlive_the_server_in_the_database_named_by_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");

    let first = connect(&db, &["--user", "samaksh"]).await;
    call(&first, "memory_remember", deployed_version_claim()).await;
    first.cancel().await.expect("close first client");

    let second = connect(&db, &["--user", "samaksh"]).await;
    let recalled = call(&second, "memory_recall", json!({ "query": "billing-api" })).await;
    assert_eq!(recalled["answer_set"][0]["scope"], "user:samaksh");
    second.cancel().await.expect("close second client");
}

#[tokio::test]
async fn an_argument_the_server_does_not_support_is_refused_not_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let client = connect(
        &dir.path().join("ledgermem.db"),
        &["--project", "billing-api"],
    )
    .await;

    let mut arguments = deployed_version_claim();
    arguments["claims"][0]["scope"] = json!("org");
    let Value::Object(arguments) = arguments else {
        unreachable!()
    };
    let refused = client
        .call_tool(CallToolRequestParams::new("memory_remember").with_arguments(arguments))
        .await
        .expect("tool call");
    assert_eq!(refused.is_error, Some(true), "{refused:?}");
    assert!(
        format!("{refused:?}").contains("unknown field `scope`"),
        "{refused:?}"
    );

    client.cancel().await.expect("close client");
}

#[tokio::test]
async fn a_stdio_server_relays_the_users_words_unless_launched_with_no_relay_user() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    apply_policy(&db, "version: 1\nextends: [personal-assistant]\n");
    let relayed = json!({ "claims": [ {
        "subject": "user:local",
        "predicate": "dietary_preference",
        "value": "vegetarian",
        "text": "the user is vegetarian",
        "relayed_user_statement": { "quote": "I'm vegetarian" }
    } ] });

    let relaying = connect(&db, &[]).await;
    let remembered = call(&relaying, "memory_remember", relayed.clone()).await;
    relaying.cancel().await.expect("close client");
    let withheld = connect(&db, &["--no-relay-user"]).await;
    let not_relayed = call(&withheld, "memory_remember", relayed).await;
    withheld.cancel().await.expect("close client");

    let result = &remembered["results"][0];
    assert_eq!(result["status"], "verified");
    assert_eq!(result["trust"]["score"], 0.80);
    assert_eq!(result["hints"], json!([]));
    let result = &not_relayed["results"][0];
    assert_eq!(result["status"], "proposed");
    assert_eq!(result["trust"]["score"], 0.30);
    assert_eq!(
        result["hints"],
        json!([
            "relayed_user_statement doesn't count: agent:agent lacks the relay_user permission, \
             so this is the agent's claim, not the user's"
        ])
    );
}

#[test]
fn a_tenant_other_than_local_must_already_exist() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ledgermem"))
        .args(["mcp", "--tenant", "acme", "--db"])
        .arg(dir.path().join("ledgermem.db"))
        .stdin(Stdio::null())
        .output()
        .expect("run ledgermem");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ledgermem tenant create acme"), "{stderr}");
}

/// A file the policy says the stdio server's working directory holds, and its SHA-256.
const CONFIG: &str = "timeout: 30\nretries: 3\n";
const CONFIG_SHA256: &str = "76e0a46cff03cdadebf4ef9bc99f861f9326775ecfb35a27950ed7c2a897bbae";

#[tokio::test]
async fn the_file_verifier_checks_claims_as_they_are_written_and_again_when_attested() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledgermem.db");
    apply_policy(
        &db,
        "version: 1
predicates:
  config_sha256:
    freshness: volatile
    system_of_record: file
    verifier: { use: file, params: { check: sha256, path: '{subject_name}' } }
",
    );
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("app.yaml"), CONFIG).unwrap();
    let mut command = mcp_command(&db, &["--project", "billing-api"]);
    command.current_dir(&work);
    let client = connect_with(command).await;

    let remembered = call(
        &client,
        "memory_remember",
        json!({ "claims": [ {
            "subject": "file:app.yaml",
            "predicate": "config_sha256",
            "value": format!("sha256:{CONFIG_SHA256}"),
            "text": "app.yaml is the reviewed configuration"
        } ] }),
    )
    .await;
    let result = &remembered["results"][0];
    assert_eq!(result["status"], "verified", "{result}");
    assert_eq!(result["verification"], "done");
    assert_eq!(result["trust"]["score"], 0.95);
    let claim_id = result["claim_id"].clone();

    std::fs::write(work.join("app.yaml"), "timeout: 5\n").unwrap();
    let attested = call(&client, "memory_attest", json!({ "claim_id": claim_id })).await;
    client.cancel().await.expect("close client");

    assert_eq!(attested["outcome"], "fail", "{attested}");
    assert_eq!(attested["status"], "retracted");
    assert_eq!(attested["reason"], "refuted");
    let evidence = &attested["evidence"][0];
    assert_eq!(evidence["kind"], "verifier");
    assert_eq!(evidence["verifier"], "file");
    assert_eq!(evidence["system_of_record"], true);
    let hash = evidence["hash"].as_str().expect("a hash");
    assert!(hash.starts_with("sha256:") && hash.len() == 71, "{hash}");
    let observed_at = evidence["observed_at"].as_str().expect("a timestamp");
    assert!(observed_at.ends_with('Z'), "{observed_at}");
}

#[tokio::test]
async fn attest_records_what_the_agent_observed_hashed_and_timestamped() {
    let dir = tempfile::tempdir().unwrap();
    let client = connect(
        &dir.path().join("ledgermem.db"),
        &["--project", "billing-api"],
    )
    .await;
    let remembered = call(
        &client,
        "memory_remember",
        json!({ "claims": [ {
            "subject": "service:billing-api",
            "predicate": "migration_status",
            "value": "applied",
            "text": "the invoices migration is applied"
        } ] }),
    )
    .await;
    let claim_id = remembered["results"][0]["claim_id"].clone();

    let attested = call(
        &client,
        "memory_attest",
        json!({ "claim_id": claim_id, "evidence": [ {
            "kind": "tool_result",
            "tool": "psql",
            "ref": "schema_migrations",
            "excerpt": "20261101_invoices | applied",
            "observed_at": "2026-11-01T09:00:00Z"
        } ] }),
    )
    .await;
    client.cancel().await.expect("close client");

    assert_eq!(attested["outcome"], "inconclusive", "{attested}");
    assert_eq!(attested["status"], "proposed");
    assert_eq!(attested["trust"]["components"]["attestation"], 0.0);
    assert_eq!(
        attested["evidence"][0],
        json!({
            "id": attested["evidence"][0]["id"],
            "kind": "tool_result",
            "tool": "psql",
            "ref": "schema_migrations",
            "excerpt": "20261101_invoices | applied",
            "hash": "sha256:5a4a02e21bb6d17746411a990fc9b1e74eb154aeba39d2e34219794ab35cbb25",
            "observed_at": "2026-11-01T09:00:00Z"
        })
    );
}
