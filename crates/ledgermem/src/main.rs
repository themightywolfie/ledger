//! The `ledgermem` binary: the CLI, and the wiring of adapters into the engine.

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use ledgermem_core::{LOCAL_TENANT, Ledger, SystemClock};
use ledgermem_server::mcp::{StdioLaunch, serve_stdio};
use ledgermem_sqlite::SqliteStore;

type BoxError = Box<dyn Error + Send + Sync>;

#[derive(Parser)]
#[command(
    version,
    about = "Ledger: the memory layer for agents that checks before it believes."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the memory tools to one agent over MCP stdio.
    Mcp(McpArgs),
}

#[derive(Args)]
struct McpArgs {
    /// The tenant to serve. `local` is created on first launch; any other must already exist.
    #[arg(long, default_value = LOCAL_TENANT)]
    tenant: String,
    /// The user scope this server is bound to.
    #[arg(long, default_value = "local")]
    user: String,
    /// The project scope this server is bound to. Without it, writes default to user scope.
    #[arg(long)]
    project: Option<String>,
    /// The actor id, and the agent scope.
    #[arg(long, default_value = "agent")]
    agent: String,
    /// Withhold relay_user, so this agent can't relay the user's own words.
    #[arg(long)]
    no_relay_user: bool,
    /// The SQLite database file. Defaults to ledgermem.db in the platform data directory.
    #[arg(long)]
    db: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Mcp(args) => mcp(args).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ledgermem: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn mcp(args: McpArgs) -> Result<(), BoxError> {
    let ledger = Arc::new(open_ledger(args.db)?);
    ledger.open_tenant(&args.tenant).await?;
    let launch = StdioLaunch {
        tenant: args.tenant,
        user: args.user,
        project: args.project,
        agent: args.agent,
        relay_user: !args.no_relay_user,
    };
    serve_stdio(ledger, launch.caller()).await
}

fn open_ledger(db: Option<PathBuf>) -> Result<Ledger, BoxError> {
    let path = match db {
        Some(path) => path,
        None => default_db_path()?,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let store = SqliteStore::open(&path)?;
    Ok(Ledger::new(Arc::new(store), Arc::new(SystemClock)))
}

/// `%APPDATA%\ledgermem\ledgermem.db`, `~/.local/share/ledgermem/ledgermem.db` or
/// `~/Library/Application Support/ledgermem/ledgermem.db`.
fn default_db_path() -> Result<PathBuf, BoxError> {
    let data_dir = dirs::data_dir().ok_or("no platform data directory found; pass --db")?;
    Ok(data_dir.join("ledgermem").join("ledgermem.db"))
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;

    use super::default_db_path;

    #[test]
    fn the_default_database_lives_in_the_platform_data_directory() {
        let home = || PathBuf::from(env::var_os("HOME").expect("HOME"));
        let data_dir = if cfg!(windows) {
            PathBuf::from(env::var_os("APPDATA").expect("APPDATA"))
        } else if cfg!(target_os = "macos") {
            home().join("Library").join("Application Support")
        } else {
            env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .unwrap_or_else(|| home().join(".local").join("share"))
        };

        assert_eq!(
            default_db_path().unwrap(),
            data_dir.join("ledgermem").join("ledgermem.db")
        );
    }
}
