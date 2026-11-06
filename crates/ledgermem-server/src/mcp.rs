//! The MCP surface: builds the caller, calls the engine, and returns the outcome as structured
//! content. Engine errors come back as tool results with `isError: true`.
//!
//! A remember or attest call always writes at once, with any slow check queued in the same write.
//! When the client announced the Tasks extension, the call then becomes a task: the task runs the
//! queued check, and the client polls `tasks/get` for the settled result. Any other client gets
//! its result at once, with the check pending for the job runner (contracts.md, section 1).

use std::error::Error as StdError;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use ledgermem_core::{
    Attest, AttestedOutcome, Caller, Ledger, Permission, Recall, Remember, RememberOutcome,
    Verification,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResponse, CallToolResult, CancelTaskParams, CreateTaskResult, GetTaskParams,
    GetTaskResult, Implementation, ServerCapabilities, ServerConfig, UpdateTaskParams,
};
use rmcp::service::RequestContext;
use rmcp::task_manager::{TaskManager, TaskOptions};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Serialize;
use serde_json::json;

const RECALL_DESCRIPTION: &str = "Recall what is known about a subject or question. Returns claims with status, trust, age and conflicts, plus guidance. Call before relying on anything from an earlier session. Treat guidance `reliable` as known; treat `unverified`, `stale` and `unresolved` as leads to check with your tools or ask about. Use intent `history` or `as_of` for questions about the past.";

const REMEMBER_DESCRIPTION: &str = "Propose claims for memory, one subject, one predicate and one value each, with a one-sentence text. Use declared predicates (resource `ledger://…/policy`). Attach evidence for what you observed through a tool (test run, commit, PR, API response): claims about your own work stay proposed until evidence backs them. For something the user told you about themselves, put their exact words in `relayed_user_statement`.";

const ATTEST_DESCRIPTION: &str = "Attach evidence to a claim, or ask Ledger to verify it now with its verifier. Call after you have checked what a proposed or stale claim says.";

/// How long a task's result is kept after the call that made it: long enough for the slowest
/// check, such as re-running a test suite, and for the client to poll after it.
const TASK_TTL: Duration = Duration::from_secs(30 * 60);

/// How often a task asks again about a check another runner holds.
const HELD_CHECK_POLL: Duration = Duration::from_millis(250);

/// The launch arguments that bind a stdio server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioLaunch {
    pub tenant: String,
    pub user: String,
    pub project: Option<String>,
    pub agent: String,
    /// False when the server was launched with `--no-relay-user`.
    pub relay_user: bool,
}

impl StdioLaunch {
    /// The caller every request on this server acts as: the agent channel, with read, propose
    /// and attest, and relay_user unless it was withheld (ADR-0009).
    pub fn caller(&self) -> Caller {
        let mut permissions = vec![Permission::Read, Permission::Propose, Permission::Attest];
        if self.relay_user {
            permissions.push(Permission::RelayUser);
        }
        let caller = Caller::agent(&self.tenant, &self.agent)
            .with_user(&self.user)
            .with_permissions(permissions);
        match &self.project {
            Some(project) => caller.with_project(project),
            None => caller,
        }
    }
}

/// Serves the memory tools over stdin and stdout until the client disconnects.
pub async fn serve_stdio(
    ledger: Arc<Ledger>,
    caller: Caller,
) -> Result<(), Box<dyn StdError + Send + Sync>> {
    let server = LedgerMcp::new(ledger, caller)
        .serve(rmcp::transport::stdio())
        .await?;
    server.waiting().await?;
    Ok(())
}

#[derive(Clone)]
pub struct LedgerMcp {
    ledger: Arc<Ledger>,
    caller: Arc<Caller>,
    /// The calls running as tasks, for clients that announced the Tasks extension.
    tasks: TaskManager,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl LedgerMcp {
    pub fn new(ledger: Arc<Ledger>, caller: Caller) -> Self {
        LedgerMcp {
            ledger,
            caller: Arc::new(caller),
            tasks: TaskManager::new(),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(name = "memory_recall", description = RECALL_DESCRIPTION)]
    async fn memory_recall(
        &self,
        Parameters(req): Parameters<Recall>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(tool_result(self.ledger.recall(&self.caller, req).await))
    }

    #[tool(name = "memory_remember", description = REMEMBER_DESCRIPTION)]
    async fn memory_remember(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(req): Parameters<Remember>,
    ) -> Result<CallToolResponse, ErrorData> {
        let outcome = self.ledger.remember(&self.caller, req).await;
        let pending = |outcome: &RememberOutcome| {
            outcome
                .results
                .iter()
                .any(|result| result.verification == Verification::Pending)
        };
        match outcome {
            Ok(outcome) if takes_tasks(&context) && pending(&outcome) => {
                let (ledger, caller) = (self.ledger.clone(), self.caller.clone());
                Ok(self.as_task(async move {
                    settled(outcome, pending, |outcome| {
                        ledger.finish_remember(&caller, outcome)
                    })
                    .await
                }))
            }
            outcome => Ok(tool_result(outcome).into()),
        }
    }

    #[tool(name = "memory_attest", description = ATTEST_DESCRIPTION)]
    async fn memory_attest(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(req): Parameters<Attest>,
    ) -> Result<CallToolResponse, ErrorData> {
        match self.ledger.attest(&self.caller, req).await {
            Ok(outcome) if takes_tasks(&context) && outcome.outcome == AttestedOutcome::Pending => {
                let (ledger, caller) = (self.ledger.clone(), self.caller.clone());
                Ok(self.as_task(async move {
                    let pending = |outcome: &ledgermem_core::AttestOutcome| {
                        outcome.outcome == AttestedOutcome::Pending
                    };
                    settled(outcome, pending, |outcome| {
                        ledger.finish_attest(&caller, outcome)
                    })
                    .await
                }))
            }
            outcome => Ok(tool_result(outcome).into()),
        }
    }
}

impl LedgerMcp {
    /// Runs `call` as a task, and returns the handle the client polls `tasks/get` with.
    fn as_task(
        &self,
        call: impl Future<Output = CallToolResult> + Send + 'static,
    ) -> CallToolResponse {
        let ttl = u64::try_from(TASK_TTL.as_millis()).unwrap_or(u64::MAX);
        let options = TaskOptions::new().with_ttl_ms(ttl);
        let task = self
            .tasks
            .spawn(options, |_| Box::pin(async move { Ok(call.await) }));
        CallToolResponse::Task(CreateTaskResult::new(task))
    }
}

/// The outcome of a call once nothing in it is pending. `finish` runs its queued checks now, and
/// a check another runner holds is asked about again until that runner is done. A runner that
/// stopped leaves the check to whoever asks once its lease runs out, so this always ends, or the
/// task's time to live ends it first.
async fn settled<T, Finishing>(
    mut outcome: T,
    pending: impl Fn(&T) -> bool,
    finish: impl Fn(T) -> Finishing,
) -> CallToolResult
where
    T: Serialize,
    Finishing: Future<Output = ledgermem_core::Result<T>>,
{
    loop {
        outcome = match finish(outcome).await {
            Ok(outcome) => outcome,
            Err(error) => return tool_result::<T>(Err(error)),
        };
        if !pending(&outcome) {
            return tool_result(Ok(outcome));
        }
        tokio::time::sleep(HELD_CHECK_POLL).await;
    }
}

/// Whether the client announced the Tasks extension on this request.
fn takes_tasks(context: &RequestContext<RoleServer>) -> bool {
    context
        .client_capabilities()
        .is_some_and(|capabilities| capabilities.supports_tasks())
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LedgerMcp {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tasks()
            .build();
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("ledgermem", env!("CARGO_PKG_VERSION")))
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.tasks
            .update_task(&request.task_id, request.input_responses)
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.tasks.cancel_task(&request.task_id)
    }
}

fn tool_result<T: Serialize>(outcome: ledgermem_core::Result<T>) -> CallToolResult {
    let value = outcome.and_then(|outcome| {
        serde_json::to_value(outcome).map_err(|e| ledgermem_core::Error::Internal(e.to_string()))
    });
    match value {
        Ok(value) => CallToolResult::structured(value),
        Err(error) => CallToolResult::structured_error(json!({
            "error": { "code": error.code(), "message": error.to_string() }
        })),
    }
}
