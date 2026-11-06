//! MCP Tasks: a call with a slow check becomes a task for a client that announced the Tasks
//! extension, and returns at once, its verification pending, for a client that didn't.

use std::sync::Arc;
use std::time::Duration;

use ledgermem_core::{
    ApplyPolicy, Caller, Channel, Ledger, Observation, Permission, ScriptedVerifier, SystemClock,
    VerifierName, Verifiers,
};
use ledgermem_server::mcp::{LedgerMcp, StdioLaunch};
use ledgermem_sqlite::SqliteStore;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientConfig, GetTaskParams,
    Implementation, JsonObject, ProtocolVersion, TaskPayload,
};
use rmcp::service::RunningService;
use rmcp::{ClientLifecycleMode, ClientServiceExt, RoleClient, ServiceExt};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, ClientConfig>;

/// A ledger whose test_run is slow, and finds billing-api's tests passing.
async fn ledger() -> Arc<Ledger> {
    let test_run = ScriptedVerifier::in_background();
    test_run.script(
        "repo:acme/billing-api",
        Observation::pass(json!("passing"), "412 of 412 tests pass"),
    );
    let verifiers = Verifiers::new().with(
        VerifierName::try_from("test_run".to_owned()).unwrap(),
        Arc::new(test_run),
    );
    let store = SqliteStore::open_in_memory().unwrap();
    let ledger = Ledger::new(Arc::new(store), Arc::new(SystemClock)).with_verifiers(verifiers);
    ledger.open_tenant("local").await.unwrap();
    let operator =
        Caller::operator("local", "test", Channel::Cli).with_permissions([Permission::Operator]);
    let policy = ApplyPolicy {
        document: "version: 1\nextends: [software-delivery]\n".into(),
        project: None,
    };
    ledger.apply_policy(&operator, policy).await.unwrap();
    Arc::new(ledger)
}

/// A client of a server for `ledger`, over an in-memory pipe, announcing `capabilities`.
async fn connect(ledger: Arc<Ledger>, capabilities: ClientCapabilities) -> Client {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let launch = StdioLaunch {
        tenant: "local".into(),
        user: "local".into(),
        project: Some("billing-api".into()),
        agent: "coder".into(),
        relay_user: true,
    };
    tokio::spawn(async move {
        let server = LedgerMcp::new(ledger, launch.caller())
            .serve(server_io)
            .await
            .expect("serve");
        let _ = server.waiting().await;
    });
    ClientConfig::new(capabilities, Implementation::new("test", "0.1.0"))
        .serve_with_lifecycle(
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("connect")
}

fn tasks() -> ClientCapabilities {
    ClientCapabilities::builder().enable_tasks().build()
}

async fn call_once(client: &Client, tool: &'static str, arguments: Value) -> CallToolResponse {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    client
        .call_tool_once(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .expect("tool call")
}

/// The structured content of a call that completed at once.
fn completed(response: CallToolResponse) -> Value {
    let CallToolResponse::Complete(result) = response else {
        panic!("expected a result at once, got {response:?}");
    };
    assert_eq!(result.is_error, Some(false), "{result:?}");
    result.structured_content.expect("structured content")
}

/// The structured content a task completed with, polling `tasks/get` until it finishes.
async fn task_result(client: &Client, response: CallToolResponse) -> Value {
    let CallToolResponse::Task(created) = response else {
        panic!("expected a task, got {response:?}");
    };
    for _ in 0..500 {
        let polled = client
            .get_task(GetTaskParams::new(created.task.task_id.clone()))
            .await
            .expect("tasks/get");
        match polled.task.payload {
            TaskPayload::Working => tokio::time::sleep(Duration::from_millis(10)).await,
            TaskPayload::Completed { result } => return structured(result),
            other => panic!("the task didn't complete: {other:?}"),
        }
    }
    panic!("the task never finished");
}

fn structured(result: JsonObject) -> Value {
    assert_eq!(result.get("isError"), Some(&json!(false)), "{result:?}");
    result["structuredContent"].clone()
}

fn tests_pass() -> Value {
    json!({ "claims": [ {
        "subject": "repo:acme/billing-api",
        "predicate": "test_status",
        "value": "passing",
        "text": "all tests pass"
    } ] })
}

#[tokio::test]
async fn a_tasks_client_gets_a_task_that_completes_with_the_checked_claim() {
    let client = connect(ledger().await, tasks()).await;

    let response = call_once(&client, "memory_remember", tests_pass()).await;
    let remembered = task_result(&client, response).await;
    client.cancel().await.expect("close client");

    let result = &remembered["results"][0];
    assert_eq!(result["status"], "verified", "{remembered}");
    assert_eq!(result["verification"], "done");
    assert_eq!(result["trust"]["score"], 0.95);
}

#[tokio::test]
async fn a_client_without_tasks_gets_its_result_at_once_with_the_check_pending() {
    let client = connect(ledger().await, ClientCapabilities::default()).await;

    let remembered = completed(call_once(&client, "memory_remember", tests_pass()).await);
    client.cancel().await.expect("close client");

    let result = &remembered["results"][0];
    assert_eq!(result["status"], "proposed", "{remembered}");
    assert_eq!(result["verification"], "pending");
}

#[tokio::test]
async fn a_call_with_no_slow_check_completes_at_once_even_for_a_tasks_client() {
    let client = connect(ledger().await, tasks()).await;

    let migration = json!({ "claims": [ {
        "subject": "service:billing-api",
        "predicate": "migration_status",
        "value": "applied",
        "text": "the invoices migration is applied"
    } ] });
    let remembered = completed(call_once(&client, "memory_remember", migration).await);
    client.cancel().await.expect("close client");

    assert_eq!(remembered["results"][0]["verification"], "none");
}

#[tokio::test]
async fn attesting_with_a_slow_check_is_a_task_too() {
    let ledger = ledger().await;
    let plain = connect(ledger.clone(), ClientCapabilities::default()).await;
    let remembered = completed(call_once(&plain, "memory_remember", tests_pass()).await);
    plain.cancel().await.expect("close client");
    let claim_id = remembered["results"][0]["claim_id"].clone();

    let client = connect(ledger, tasks()).await;
    let response = call_once(&client, "memory_attest", json!({ "claim_id": claim_id })).await;
    let attested = task_result(&client, response).await;
    client.cancel().await.expect("close client");

    assert_eq!(attested["outcome"], "pass", "{attested}");
    assert_eq!(attested["status"], "verified");
}
