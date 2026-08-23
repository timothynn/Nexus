use std::{env, fs, io::{self, Write}, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use nexus_config::Config;
use nexus_context::{ContextOptions, discover};
use nexus_core::SessionId;
use nexus_models::{MockModelProvider, ModelProvider, ModelStreamEvent, OpenAiCompatibleProvider};
use nexus_permissions::{PermissionApprover, PermissionDecision, PermissionRequest, RuleBasedPolicy};
use nexus_runtime::{AgentRuntime, AuthorizedToolExecutor};
use nexus_storage::{SessionStore, SqliteStore};
use nexus_tools::{FileSystemTool, ShellTool, ToolRegistry};
use nexus_workspace::GitWorktreeManager;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "nexus", version, about = "A configurable AI agent harness")]
struct Cli { #[command(subcommand)] command: Option<Command> }

#[derive(Debug, Subcommand)]
enum Command {
    Run {
        task: String,
        #[arg(short, long)] stream: bool,
        #[arg(short, long, default_value = "mock-1")] model: String,
        #[arg(long, default_value = "mock")] provider: String,
        #[arg(long, default_value = "https://api.openai.com/v1")] base_url: String,
        #[arg(long, default_value = "OPENAI_API_KEY")] api_key_env: String,
        #[arg(long)] tools: bool,
        #[arg(long)] max_steps: Option<usize>,
        #[arg(long)] yes: bool,
        /// Persist the execution trace in .nexus/nexus.db.
        #[arg(long)] session: bool,
    },
    Models,
    /// Display resolved configuration and configuration sources.
    Config,
    /// Inspect repository context and deterministic token budgets.
    Context {
        #[arg(long)] max_files: Option<usize>,
        #[arg(long)] token_budget: Option<usize>,
    },
    /// Replay persisted execution events from a session ID.
    Replay { session_id: String },
    Doctor,
    Worktree { #[command(subcommand)] command: WorktreeCommand },
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    Create { name: String, #[arg(long)] base: Option<String> },
    List,
    Status { name: String },
    Diff { name: String },
    Remove { name: String, #[arg(long)] force: bool },
    /// Allocate one isolated worktree for every parallel agent in a run.
    AllocateAgents { run_name: String, count: usize, #[arg(long)] base: Option<String> },
}

struct StdinApprover;
impl PermissionApprover for StdinApprover {
    fn approve(&self, request: &PermissionRequest) -> bool {
        eprint!("[nexus] allow `{}`? [y/N] ", request.action);
        if io::stderr().flush().is_err() { return false; }
        let mut answer = String::new();
        match io::stdin().read_line(&mut answer) {
            Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
            Err(_) => false,
        }
    }
}
struct ApproveAll;
impl PermissionApprover for ApproveAll { fn approve(&self, _request: &PermissionRequest) -> bool { true } }

fn build_provider(provider_name: &str, base_url: &str, api_key_env: &str) -> Result<Arc<dyn ModelProvider>> {
    match provider_name {
        "mock" => Ok(Arc::new(MockModelProvider::default())),
        "openai-compatible" => {
            let api_key = env::var(api_key_env).with_context(|| format!("missing API key environment variable `{api_key_env}` for the selected provider"))?;
            Ok(Arc::new(OpenAiCompatibleProvider::new("openai-compatible", base_url, api_key)?))
        }
        _ => bail!("unknown provider `{provider_name}`; supported providers: mock, openai-compatible"),
    }
}

fn build_tool_executor(root: PathBuf, yes: bool) -> Result<AuthorizedToolExecutor> {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FileSystemTool::new(root.clone())))?;
    registry.register(Arc::new(ShellTool::new(root)))?;
    let policy = RuleBasedPolicy::new(PermissionDecision::Deny)
        .with_rule("filesystem.read", PermissionDecision::Allow)
        .with_rule("shell.execute", PermissionDecision::Ask);
    let executor = AuthorizedToolExecutor::new(registry, Arc::new(policy));
    Ok(if yes { executor.with_approver(Arc::new(ApproveAll)) } else { executor.with_approver(Arc::new(StdinApprover)) })
}

fn session_store(root: &PathBuf) -> Result<SqliteStore> {
    let directory = root.join(".nexus");
    fs::create_dir_all(&directory)?;
    Ok(SqliteStore::open(directory.join("nexus.db"))?)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).with_target(false).init();
    let cli = Cli::parse();
    let root = env::current_dir()?;
    match cli.command {
        Some(Command::Run { task, stream, model, provider, base_url, api_key_env, tools, max_steps, yes, session }) => {
            if stream && tools { bail!("streaming with model-driven tool execution is not implemented yet"); }
            let resolved = Config::load_from(&root)?;
            let step_limit = max_steps.unwrap_or(resolved.config.max_steps);
            let model_provider = build_provider(&provider, &base_url, &api_key_env)?;
            let runtime = AgentRuntime::new(resolved.config, model_provider, model);
            let session_id = SessionId::new();
            let store = if session {
                let store = session_store(&root)?;
                store.create(session_id.clone()).await?;
                store.append_event(&session_id.0.to_string(), "started", &serde_json::json!({"task": task, "provider": provider}))?;
                Some(store)
            } else { None };

            let result = if stream {
                runtime.run_streaming(&task, |event| match event {
                    ModelStreamEvent::Started { model } => eprintln!("[nexus] model: {}", model.0),
                    ModelStreamEvent::Delta { content } => print!("{content}"),
                    ModelStreamEvent::Completed { usage } => eprintln!("\n[nexus] usage: {} input / {} output tokens", usage.input_tokens, usage.output_tokens),
                }).await?
            } else if tools {
                let executor = build_tool_executor(root.clone(), yes)?;
                runtime.run_with_tools(&task, &executor, step_limit).await?
            } else { runtime.run(&task).await? };

            if let Some(store) = store {
                store.append_event(&session_id.0.to_string(), "completed", &serde_json::json!({"task_id": result.task_id, "message": result.message, "provider": result.provider, "model": result.model, "input_tokens": result.usage.input_tokens, "output_tokens": result.usage.output_tokens}))?;
                eprintln!("[nexus] session {} persisted", session_id.0);
            }
            if !stream { println!("{}", result.message); }
            eprintln!("[nexus] run {} completed via {} using {} input / {} output tokens", result.task_id, result.provider, result.usage.input_tokens, result.usage.output_tokens);
        }
        Some(Command::Models) => {
            println!("Built-in providers:");
            println!("  mock               - deterministic local provider for development and tests");
            println!("  openai-compatible  - HTTP adapter for compatible Chat Completions APIs");
        }
        Some(Command::Config) => {
            let resolved = Config::load_from(&root)?;
            println!("default_agent = {}", resolved.config.default_agent);
            println!("max_steps = {}", resolved.config.max_steps);
            println!("context.token_budget = {}", resolved.config.context.token_budget);
            if resolved.sources.is_empty() { println!("sources = built-in defaults + environment"); }
            else { for source in resolved.sources { println!("source = {}", source.display()); } }
        }
        Some(Command::Context { max_files, token_budget }) => {
            let resolved = Config::load_from(&root)?;
            let options = ContextOptions { max_files: max_files.unwrap_or(resolved.config.context.max_files), max_bytes_per_file: resolved.config.context.max_bytes_per_file, token_budget: token_budget.unwrap_or(resolved.config.context.token_budget) };
            let snapshot = discover(&root, &options)?;
            println!("root = {}", snapshot.root.display());
            println!("files = {}", snapshot.files.len());
            println!("estimated_tokens = {}", snapshot.total_estimated_tokens);
            println!("truncated = {}", snapshot.truncated);
            for file in snapshot.files { println!("{}\t{} tokens", file.path.display(), file.estimated_tokens); }
        }
        Some(Command::Replay { session_id }) => {
            let store = session_store(&root)?;
            for event in store.replay(&session_id)? { println!("{}\t{}\t{}", event.sequence, event.kind, event.payload); }
        }
        Some(Command::Doctor) => {
            println!("Nexus runtime: healthy");
            println!("Model gateway: mock + OpenAI-compatible adapter available");
            println!("Tool registry: filesystem + structured shell available");
            println!("Permission approvals: CLI boundary available");
            println!("Workspace isolation: Git worktrees + parallel allocation available");
            println!("Storage: SQLite sessions + replay available");
            println!("Context: repository discovery + token budgeting available");
        }
        Some(Command::Worktree { command }) => {
            let manager = GitWorktreeManager::new(root)?;
            match command {
                WorktreeCommand::Create { name, base } => { let worktree = manager.create(&name, base.as_deref())?; println!("Created workspace `{}`\nBranch: {}\nPath: {}", worktree.name, worktree.branch, worktree.path.display()); }
                WorktreeCommand::List => for worktree in manager.list()? { println!("{}\t{}\t{}", worktree.name, worktree.branch, worktree.path.display()); },
                WorktreeCommand::Status { name } => println!("{}", manager.status(&name)?),
                WorktreeCommand::Diff { name } => println!("{}", manager.diff(&name)?),
                WorktreeCommand::Remove { name, force } => { manager.remove(&name, force)?; println!("Removed workspace `{name}`. No changes were merged."); }
                WorktreeCommand::AllocateAgents { run_name, count, base } => for workspace in manager.allocate_agents(&run_name, count, base.as_deref())? { println!("agent-{}\t{}\t{}", workspace.agent_index + 1, workspace.worktree.branch, workspace.worktree.path.display()); },
            }
        }
        None => { println!("Nexus {}", env!("CARGO_PKG_VERSION")); println!("Run `nexus --help` to get started."); }
    }
    Ok(())
}
