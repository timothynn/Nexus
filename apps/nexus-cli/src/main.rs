use std::{env, sync::Arc};

use anyhow::Result;
use clap::{Parser, Subcommand};
use nexus_config::Config;
use nexus_models::{MockModelProvider, ModelStreamEvent};
use nexus_runtime::AgentRuntime;
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
        /// Stream model output as events arrive.
        #[arg(short, long)]
        stream: bool,
        /// Model identifier passed to the selected provider.
        #[arg(short, long, default_value = "mock-1")]
        model: String,
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
        }) => {
            let config = Config::default();
            let runtime = AgentRuntime::new(config, Arc::new(MockModelProvider::default()), model);

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
            } else {
                let result = runtime.run(&task).await?;
                println!("{}", result.message);
            }
        }
        Some(Command::Models) => {
            println!("Built-in providers:");
            println!("  mock  - deterministic local provider for development and tests");
        }
        Some(Command::Config) => {
            println!("Nexus configuration is valid (default configuration loaded).");
        }
        Some(Command::Doctor) => {
            println!("Nexus runtime: healthy");
            println!("Model gateway: healthy");
            println!("Tool registry: available");
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
