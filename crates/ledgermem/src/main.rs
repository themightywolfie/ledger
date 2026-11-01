//! The `ledgermem` binary: the CLI, and the wiring of adapters into the engine.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use ledgermem_core::{
    ApplyPolicy, Caller, ChainReport, Channel, LOCAL_TENANT, Ledger, Permission, PolicyApplied,
    PolicyQuery, PredicatePack, ReindexReport, SystemClock,
};
use ledgermem_server::mcp::{StdioLaunch, serve_stdio};
use ledgermem_sqlite::SqliteStore;

type BoxError = Box<dyn Error + Send + Sync>;

/// How `verify` exits when a journal doesn't verify. Errors exit 1 and usage errors exit 2.
const EXIT_CHAIN_BROKEN: u8 = 3;

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
    /// Check that every journal in the tenant still verifies. Exits 3 if any is broken.
    Verify(OperatorArgs),
    /// Rebuild the tenant's claims and recall index from its journals.
    Reindex(OperatorArgs),
    /// Apply or show the tenant's policy, or list the predicate packs a policy can extend.
    #[command(subcommand)]
    Policy(PolicyCommand),
}

#[derive(Subcommand)]
enum PolicyCommand {
    /// Apply a policy file to the tenant, or to one project as an overlay on the tenant's policy.
    Apply {
        /// The policy document, written in YAML.
        file: PathBuf,
        /// The project to overlay.
        #[arg(long)]
        project: Option<String>,
        #[command(flatten)]
        operator: OperatorArgs,
    },
    /// Show the policy in effect for the tenant, or for one project with its overlay.
    Show {
        /// The project whose overlay to include.
        #[arg(long)]
        project: Option<String>,
        #[command(flatten)]
        operator: OperatorArgs,
    },
    /// List the predicate packs a policy can extend.
    Packs {
        /// Print the packs as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// Commands an operator runs on the database they open.
#[derive(Args)]
struct OperatorArgs {
    /// The tenant to work on.
    #[arg(long, default_value = LOCAL_TENANT)]
    tenant: String,
    /// The SQLite database file. Defaults to ledgermem.db in the platform data directory.
    #[arg(long)]
    db: Option<PathBuf>,
    /// Print the result as JSON.
    #[arg(long)]
    json: bool,
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
        Command::Mcp(args) => mcp(args).await.map(|()| ExitCode::SUCCESS),
        Command::Verify(args) => verify(args).await,
        Command::Reindex(args) => reindex(args).await,
        Command::Policy(PolicyCommand::Apply {
            file,
            project,
            operator,
        }) => apply_policy(&file, project, operator).await,
        Command::Policy(PolicyCommand::Show { project, operator }) => {
            show_policy(project, operator).await
        }
        Command::Policy(PolicyCommand::Packs { json }) => list_packs(json),
    };
    match result {
        Ok(code) => code,
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

async fn verify(args: OperatorArgs) -> Result<ExitCode, BoxError> {
    let (ledger, operator) = open_as_operator(&args).await?;
    let report = ledger.verify_journals(&operator).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_chain_report(&args.tenant, &report);
    }
    Ok(if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_CHAIN_BROKEN)
    })
}

async fn reindex(args: OperatorArgs) -> Result<ExitCode, BoxError> {
    let (ledger, operator) = open_as_operator(&args).await?;
    let report = ledger.reindex(&operator).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_reindex_report(&args.tenant, &report);
    }
    Ok(ExitCode::SUCCESS)
}

async fn apply_policy(
    file: &Path,
    project: Option<String>,
    args: OperatorArgs,
) -> Result<ExitCode, BoxError> {
    let document = std::fs::read_to_string(file)
        .map_err(|error| format!("can't read {}: {error}", file.display()))?;
    let (ledger, operator) = open_as_operator(&args).await?;
    let applied = ledger
        .apply_policy(&operator, ApplyPolicy { document, project })
        .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&applied)?);
    } else {
        print_policy_applied(&applied);
    }
    Ok(ExitCode::SUCCESS)
}

async fn show_policy(project: Option<String>, args: OperatorArgs) -> Result<ExitCode, BoxError> {
    let (ledger, operator) = open_as_operator(&args).await?;
    let policy = ledger.policy(&operator, PolicyQuery { project }).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&policy)?);
    } else {
        print!("{}", serde_yaml::to_string(&policy)?);
    }
    Ok(ExitCode::SUCCESS)
}

fn list_packs(json: bool) -> Result<ExitCode, BoxError> {
    let packs = PredicatePack::all();
    if json {
        println!("{}", serde_json::to_string_pretty(&packs)?);
        return Ok(ExitCode::SUCCESS);
    }
    let width = packs.iter().map(|pack| pack.name.len()).max().unwrap_or(0);
    for pack in &packs {
        let predicates: Vec<_> = pack.predicates.keys().map(String::as_str).collect();
        println!("{:width$}  {}", pack.name, predicates.join(", "));
    }
    Ok(ExitCode::SUCCESS)
}

/// The ledger behind `args`, and the local operator who runs commands on it.
async fn open_as_operator(args: &OperatorArgs) -> Result<(Ledger, Caller), BoxError> {
    let ledger = open_ledger(args.db.clone())?;
    ledger.open_tenant(&args.tenant).await?;
    let operator = Caller::operator(&args.tenant, "local", Channel::Cli).with_permissions([
        Permission::Read,
        Permission::Operator,
        Permission::Admin,
    ]);
    Ok((ledger, operator))
}

fn print_chain_report(tenant: &str, report: &ChainReport) {
    if report.scopes.is_empty() {
        println!("Tenant {tenant} has no journals yet.");
        return;
    }
    let scopes: Vec<String> = report.scopes.iter().map(|c| c.scope.to_string()).collect();
    let width = scopes.iter().map(String::len).max().unwrap_or(0);
    for (scope, chain) in scopes.iter().zip(&report.scopes) {
        match &chain.broken_at {
            None => println!(
                "{scope:width$}  ok  {}  head {}",
                plural(chain.entries, "entry", "entries"),
                chain.head_hash.as_deref().unwrap_or("-"),
            ),
            Some(broken) => println!(
                "{scope:width$}  BROKEN at entry {}: {}",
                broken.seq, broken.problem
            ),
        }
    }
}

fn print_reindex_report(tenant: &str, report: &ReindexReport) {
    println!(
        "Rebuilt tenant {tenant} from {} ({}): {}.",
        plural(report.scopes, "journal", "journals"),
        plural(report.entries, "entry", "entries"),
        plural(report.claims, "claim", "claims"),
    );
}

fn print_policy_applied(applied: &PolicyApplied) {
    println!(
        "Applied policy {} to {}: {}, {}.",
        applied.policy_version,
        applied.scope,
        plural(applied.predicates, "predicate", "predicates"),
        plural(applied.aliases, "alias", "aliases"),
    );
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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
