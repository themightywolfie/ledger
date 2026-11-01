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
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ledgermem"));
    command.arg("mcp").arg("--db").arg(db).args(args);
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
    assert_eq!(tools, ["memory_recall", "memory_remember"]);

    let remembered = call(&client, "memory_remember", deployed_version_claim()).await;
    let result = &remembered["results"][0];
    assert_eq!(result["status"], "proposed");
    assert_eq!(result["trust"], 0.30);
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
    assert_eq!(answer["trust"], 0.30);
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
    let policy = dir.path().join("ledgermem.policy.yaml");
    std::fs::write(&policy, "version: 1\nextends: [personal-assistant]\n").unwrap();
    let applied = std::process::Command::new(env!("CARGO_BIN_EXE_ledgermem"))
        .args(["policy", "apply"])
        .arg(&policy)
        .arg("--db")
        .arg(&db)
        .stdin(Stdio::null())
        .output()
        .expect("run ledgermem");
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
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
    assert_eq!(result["trust"], 0.80);
    assert_eq!(result["hints"], json!([]));
    let result = &not_relayed["results"][0];
    assert_eq!(result["status"], "proposed");
    assert_eq!(result["trust"], 0.30);
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
