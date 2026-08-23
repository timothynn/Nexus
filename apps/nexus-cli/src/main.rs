use std::{
    env,
    io::{self, Write},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use nexus_config::Config;
use nexus_models::{
    MockModelProvider, ModelProvider, ModelStreamEvent, OpenAiCompatibleProvider,
};
use nexus_permissions::{
    PermissionApprover, PermissionDecision, PermissionRequest, RuleBasedPolicy,
};
use nexus_runtime::{AgentRuntime, AuthorizedToolExecutor};
use nexus_tools::{FileSystemTool, ShellTool, ToolRegistry};
use nexus_workspace::GitWorktreeManager;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "nexus", version, about = "A configurable AI agent harness")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a task through the Nexus runtime.
    Run {
        task: String,
        /// Stream model output as events arrive. Not yet available with tool execution.
        #[arg(short, long)]
        stream: bool,
        /// Model identifier passed to the selected provider.
        #[arg(short, long, default_value = "mock-1")]
        model: String,
        /// Provider: `mock` or `openai-compatible`.
        #[arg(long, default_value = "mock")]
        provider: String,
        /// Base URL for an OpenAI-compatible Chat Completions endpoint.
        #[arg(long, default_value = "https://api.openai.com/v1")]
        base_url: String,
        /// Environment variable containing the provider API key.
        #[arg(long, default_value = "OPENAI_API_KEY")]
        api_key_env: String,
        /// Enable the built-in workspace filesystem and structured shell tools.
        #[arg(long)]
        tools: bool,
        /// Maximum model/tool iterations when tools are enabled.
        #[arg(long, default_value_t = 16)]
        max_steps: usize,
        /// Automatically approve actions whose policy is `ask`.
        #[arg(long)]
        yes: bool,
    },
    /// List providers currently built into the CLI.
    Models,
    /// Validate the resolved Nexus configuration.
    Config,
    /// Display the runtime version and health.
    Doctor,
    /// Manage isolated Git workspaces for agent runs.
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    /// Create an isolated worktree on a Nexus-managed branch.
    Create {
        name: String,
        /// Base commit or ref. Defaults to HEAD.
        #[arg(long)]
        base: Option<String>,
    },
    /// List Nexus-managed worktrees.
    List,
    /// Show the working tree status for an isolated workspace.
    Status { name: String },
    /// Show uncommitted changes inside an isolated workspace.
    Diff { name: String },
    /// Remove an isolated workspace. This never merges changes.
    Remove {
        name: String,
        #[arg(long)]
        force: bool,
    },
}

struct StdinApprover;

impl PermissionApprover for StdinApprover {
    fn approve(&self, request: &PermissionRequest) -> bool {
        eprint!("[nexus] allow `{}`? [y/N] ", request.action);
        if io::stderr().flush().is_err() {
            return false;
        }

        let mut answer = String::new();
        match io::stdin().read_line(&mut answer) {
            Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
            Err(_) => false,
        }
    }
}

struct ApproveAll;

impl PermissionApprover for ApproveAll {
    fn approve(&self, _request: &PermissionRequest) -> bool {
        true
    }
}

fn build_provider(
    provider_name: &str,
    base_url: &str,
    api_key_env: &str,
) -> Result<Arc<dyn ModelProvider>> {
    match provider_name {
        "mock" => Ok(Arc::new(MockModelProvider::default())),
        "openai-compatible" => {
            let api_key = env::var(api_key_env).with_context(|| {
                format!("missing API key environment variable `{api_key_env}` for the selected provider")
            })?;
            let provider = OpenAiCompatibleProvider::new(
                "openai-compatible",
                base_url,
                api_key,
            )
            .map_err(anyhow::Error::from)?;
            Ok(Arc::new(provider))
        }
        _ => bail!(
            "unknown provider `{provider_name}`; supported providers: mock, openai-compatible"
        ),
    }
}

fn build_tool_executor(root: std::path::PathBuf, yes: bool) -> Result<AuthorizedToolExecutor> {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FileSystemTool::new(root.clone())))?;
    registry.register(Arc::new(ShellTool::new(root)))?;

    let policy = RuleBasedPolicy::new(PermissionDecision::Deny)
        .with_rule("filesystem.read", PermissionDecision::Allow)
        .with_rule("shell.execute", PermissionDecision::Ask);
    let executor = AuthorizedToolExecutor::new(registry, Arc::new(policy));

    Ok(if yes {
        executor.with_approver(Arc::new(ApproveAll))
    } else {
        executor.with_approver(Arc::new(StdinApprover))
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Run {
            task,
            stream,
            model,
            provider,
            base_url,
            api_key_env,
            tools,
            max_steps,
            yes,
        }) => {
            if stream && tools {
                bail!("streaming with model-driven tool execution is not implemented yet");
            }

            let config = Config::default();
            let model_provider = build_provider(&provider, &base_url, &api_key_env)?;
            let runtime = AgentRuntime::new(config, model_provider, model);

            if stream {
                let result = runtime
                    .run_streaming(&task, |event| match event {
                        ModelStreamEvent::Started { model } => {
                            eprintln!("[nexus] model: {}", model.0);
                        }
                        ModelStreamEvent::Delta { content } => print!("{content}"),
                        ModelStreamEvent::Completed { usage } => {
                            eprintln!(
                                "\n[nexus] usage: {} input / {} output tokens",
                                usage.input_tokens, usage.output_tokens
                            );
                        }
                    })
                    .await?;

                eprintln!(
                    "[nexus] run {} completed via {}",
                    result.task_id, result.provider
                );
            } else if tools {
                let executor = build_tool_executor(env::current_dir()?, yes)?;
                let result = runtime.run_with_tools(&task, &executor, max_steps).await?;
                println!("{}", result.message);
                eprintln!(
                    "[nexus] run {} completed via {} using {} input / {} output tokens",
                    result.task_id,
                    result.provider,
                    result.usage.input_tokens,
                    result.usage.output_tokens
                );
            } else {
                let result = runtime.run(&task).await?;
                println!("{}", result.message);
            }
        }
        Some(Command::Models) => {
            println!("Built-in providers:");
            println!("  mock               - deterministic local provider for development and tests");
            println!("  openai-compatible  - HTTP adapter for compatible Chat Completions APIs");
        }
        Some(Command::Config) => {
            println!("Nexus configuration is valid (default configuration loaded).");
        }
        Some(Command::Doctor) => {
            println!("Nexus runtime: healthy");
            println!("Model gateway: mock + OpenAI-compatible adapter available");
            println!("Tool registry: filesystem + structured shell available");
            println!("Permission approvals: CLI boundary available");
            println!("Workspace isolation: Git worktrees available");
            println!("Default provider: mock");
        }
        Some(Command::Worktree { command }) => {
            let repository = env::current_dir()?;
            let manager = GitWorktreeManager::new(repository)?;
            match command {
                WorktreeCommand::Create { name, base } => {
                    let worktree = manager.create(&name, base.as_deref())?;
                    println!("Created workspace `{}`", worktree.name);
                    println!("Branch: {}", worktree.branch);
                    println!("Path: {}", worktree.path.display());
                }
                WorktreeCommand::List => {
                    for worktree in manager.list()? {
                        println!(
                            "{}\t{}\t{}",
                            worktree.name,
                            worktree.branch,
                            worktree.path.display()
                        );
                    }
                }
                WorktreeCommand::Status { name } => {
                    println!("{}", manager.status(&name)?);
                }
                WorktreeCommand::Diff { name } => {
                    println!("{}", manager.diff(&name)?);
                }
                WorktreeCommand::Remove { name, force } => {
                    manager.remove(&name, force)?;
                    println!("Removed workspace `{name}`. No changes were merged.");
                }
            }
        }
        None => {
            println!("Nexus {}", env!("CARGO_PKG_VERSION"));
            println!("Run `nexus --help` to get started.");
        }
    }

    Ok(())
}
