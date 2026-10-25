//! The MCP surface: builds the caller, calls the engine, and returns the outcome as structured
//! content. Engine errors come back as tool results with `isError: true`.

use std::error::Error as StdError;
use std::sync::Arc;

use ledgermem_core::{Caller, Ledger, Permission, Recall, Remember};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Serialize;
use serde_json::json;

const RECALL_DESCRIPTION: &str = "Recall what is known about a subject or question. Returns claims with status, trust, age and conflicts, plus guidance. Call before relying on anything from an earlier session. Treat guidance `reliable` as known; treat `unverified`, `stale` and `unresolved` as leads to check with your tools or ask about. Use intent `history` or `as_of` for questions about the past.";

const REMEMBER_DESCRIPTION: &str = "Propose claims for memory, one subject, one predicate and one value each, with a one-sentence text. Use declared predicates (resource `ledger://…/policy`). Attach evidence for what you observed through a tool (test run, commit, PR, API response): claims about your own work stay proposed until evidence backs them. For something the user told you about themselves, put their exact words in `relayed_user_statement`.";

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
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl LedgerMcp {
    pub fn new(ledger: Arc<Ledger>, caller: Caller) -> Self {
        LedgerMcp {
            ledger,
            caller: Arc::new(caller),
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
        Parameters(req): Parameters<Remember>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(tool_result(self.ledger.remember(&self.caller, req).await))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LedgerMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("ledgermem", env!("CARGO_PKG_VERSION")))
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
