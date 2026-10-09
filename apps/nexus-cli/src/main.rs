// Command-line flag structs are naturally bool-heavy.
#![allow(clippy::struct_excessive_bools)]

mod agents_cmd;
mod chat;
mod mission_cmd;
mod support;
mod teams_cmd;

use std::{env, path::Path, path::PathBuf, process::Command as Process};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use nexus_config::Config;
use nexus_context::{
    CodeIndex, ContextOptions, discover, discover_git_aware, discover_instructions,
};
use nexus_mcp::{McpServerCommand, StdioMcpClient};
use nexus_models::ModelStreamEvent;
use nexus_runtime::AgentRuntime;
use nexus_skills::{HookEvent, discover_skills, load_agent_template, load_hooks, load_skill};
use nexus_workspace::GitWorktreeManager;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use crate::{
    agents_cmd::AgentsCommand,
    support::{
        Approval, ProviderArgs, WORKSPACE_TOOLS, build_tool_executor, resolved_instructions,
        session_store,
    },
    teams_cmd::{BotsCommand, TeamCommand},
};

#[derive(Debug, Parser)]
#[command(name = "nexus", version, about = "A configurable AI agent harness")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a single agent task.
    Run(Box<RunArgs>),
    /// List supported model providers.
    Models,
    /// Print the resolved configuration.
    Config,
    /// Inspect repository context selection.
    Context {
        #[arg(long)]
        max_files: Option<usize>,
        #[arg(long)]
        token_budget: Option<usize>,
        #[arg(long)]
        git_aware: bool,
    },
    /// Search the local code index.
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Print composed instructions for a path.
    Instructions {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Parallel and graph-orchestrated agents in isolated worktrees.
    Agents {
        #[command(subcommand)]
        command: AgentsCommand,
    },
    /// Manage bots (team members).
    Bots {
        #[command(subcommand)]
        command: BotsCommand,
    },
    /// Build teams of bots and run them toward goals.
    Team {
        #[command(subcommand)]
        command: TeamCommand,
    },
    /// Replay a persisted event stream.
    Replay { session_id: String },
    /// Check the local environment.
    Doctor,
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
}

#[derive(Debug, clap::Args)]
struct RunArgs {
    task: String,
    #[arg(short, long)]
    stream: bool,
    #[command(flatten)]
    provider: ProviderArgs,
    #[arg(long)]
    tools: bool,
    #[arg(long)]
    max_steps: Option<usize>,
    #[arg(long)]
    yes: bool,
    #[arg(long)]
    git_context: bool,
    #[arg(long)]
    agent_template: Option<String>,
    #[arg(long = "skill")]
    skills: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum SkillsCommand {
    List,
    Show { name: String },
    Hooks { event: Option<String> },
    Template { name: String },
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    ListTools {
        program: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    Call {
        program: String,
        tool: String,
        arguments: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    Create {
        name: String,
        #[arg(long)]
        base: Option<String>,
    },
    List,
    Status {
        name: String,
    },
    Diff {
        name: String,
    },
    /// Show the status and diff a human must review before merging.
    Review {
        name: String,
    },
    /// Merge a reviewed worktree branch into the current branch.
    Merge {
        name: String,
        /// Required: confirms a human reviewed the changes.
        #[arg(long)]
        approve: bool,
    },
    Remove {
        name: String,
        #[arg(long)]
        force: bool,
    },
    AllocateAgents {
        run_name: String,
        count: usize,
        #[arg(long)]
        base: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();
    let cli = Cli::parse();
    let root = env::current_dir()?;
    match cli.command {
        Some(Command::Run(args)) => run_task(&root, *args).await,
        Some(Command::Agents { command }) => agents_cmd::run(&root, command).await,
        Some(Command::Bots { command }) => teams_cmd::run_bots(&root, command),
        Some(Command::Team { command }) => teams_cmd::run_team(&root, command).await,
        Some(Command::Skills { command }) => skills(&root, command),
        Some(Command::Mcp { command }) => mcp(command).await,
        Some(Command::Worktree { command }) => worktree(&root, command),
        Some(command) => inspect(&root, command),
        None => {
            println!("Nexus: use `nexus --help` to inspect available commands.");
            Ok(())
        }
    }
}

async fn run_task(root: &Path, args: RunArgs) -> Result<()> {
    if args.stream && args.tools {
        bail!("streaming with model-driven tool execution is not implemented yet");
    }
    let resolved = Config::load_from(root)?;
    let step_limit = args.max_steps.unwrap_or(resolved.config.max_steps);
    let runtime = AgentRuntime::new(
        resolved.config,
        args.provider.build()?,
        args.provider.model.clone(),
    );
    if args.tools {
        let approval = if args.yes {
            Approval::Always
        } else {
            Approval::Prompt
        };
        let executor = build_tool_executor(root, &WORKSPACE_TOOLS, approval)?;
        let instructions = resolved_instructions(
            root,
            root,
            args.agent_template.as_deref(),
            &args.skills,
            args.git_context,
        )?;
        let result = runtime
            .run_with_tools_controlled_with_instructions(
                &args.task,
                Some(&instructions),
                &executor,
                step_limit,
                CancellationToken::new(),
                None,
            )
            .await?;
        println!("{}", result.message);
    } else if args.stream {
        let result = runtime
            .run_streaming(&args.task, |event| {
                if let ModelStreamEvent::Delta { content } = event {
                    print!("{content}");
                }
            })
            .await?;
        println!();
        eprintln!(
            "\n[usage] input={} output={} total={}",
            result.usage.input_tokens,
            result.usage.output_tokens,
            result.usage.input_tokens + result.usage.output_tokens
        );
    } else {
        println!("{}", runtime.run(&args.task).await?.message);
    }
    Ok(())
}

fn inspect(root: &Path, command: Command) -> Result<()> {
    match command {
        Command::Models => println!("mock\nopenai-compatible"),
        Command::Config => println!("{:#?}", Config::load_from(root)?.config),
        Command::Context {
            max_files,
            token_budget,
            git_aware,
        } => {
            let mut options = ContextOptions::default();
            if let Some(max_files) = max_files {
                options.max_files = max_files;
            }
            if let Some(token_budget) = token_budget {
                options.token_budget = token_budget;
            }
            let (snapshot, prioritized) = if git_aware {
                let aware = discover_git_aware(root, &options)?;
                (aware.snapshot, aware.prioritized_files)
            } else {
                (discover(root, &options)?, Vec::new())
            };
            println!(
                "files={} estimated_tokens={} truncated={} prioritized={}",
                snapshot.files.len(),
                snapshot.total_estimated_tokens,
                snapshot.truncated,
                prioritized.len()
            );
        }
        Command::Search { query, limit } => {
            let index = CodeIndex::build(&discover(root, &ContextOptions::default())?);
            for result in index.search(&query, limit) {
                println!(
                    "{}:{}  [{}] {}",
                    result.path.display(),
                    result.line,
                    result.score,
                    result.text
                );
            }
        }
        Command::Instructions { path } => {
            println!("{}", discover_instructions(root, &path, None)?.combined());
        }
        Command::Replay { session_id } => {
            for event in session_store(root)?.replay(&session_id)? {
                println!("{} {} {}", event.sequence, event.kind, event.payload);
            }
        }
        Command::Doctor => doctor(root),
        _ => unreachable!("dispatched in main"),
    }
    Ok(())
}

fn doctor(root: &Path) {
    let check = |label: &str, ok: bool, detail: String| {
        println!("{} {label:<14} {detail}", if ok { "✔" } else { "✖" });
    };
    check("workspace", true, root.display().to_string());
    let git = Process::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(root)
        .output();
    match git {
        Ok(output) if output.status.success() => check(
            "git",
            true,
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ),
        _ => check(
            "git",
            false,
            "not a Git repository (worktrees and agents need one)".to_owned(),
        ),
    }
    match Config::load_from(root) {
        Ok(resolved) => check(
            "config",
            true,
            if resolved.sources.is_empty() {
                "defaults".to_owned()
            } else {
                resolved
                    .sources
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ),
        Err(error) => check("config", false, error.to_string()),
    }
    check(
        "openai key",
        env::var_os("OPENAI_API_KEY").is_some(),
        "OPENAI_API_KEY (needed for --provider openai-compatible)".to_owned(),
    );
    let docker = Process::new("docker").arg("--version").output();
    check(
        "containers",
        docker.as_ref().is_ok_and(|output| output.status.success()),
        docker.ok().map_or_else(
            || "docker not found".to_owned(),
            |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ),
    );
    let library = nexus_teams::TeamLibrary::new(root);
    check(
        "teams",
        true,
        format!(
            "{} bots, {} teams in .nexus/",
            library.list_bots().map_or(0, |bots| bots.len()),
            library.list_teams().map_or(0, |teams| teams.len())
        ),
    );
}

fn skills(root: &Path, command: SkillsCommand) -> Result<()> {
    match command {
        SkillsCommand::List => {
            for skill in discover_skills(root)? {
                println!("{}\t{}", skill.name, skill.description);
            }
        }
        SkillsCommand::Show { name } => println!("{}", load_skill(root, &name)?.instructions),
        SkillsCommand::Template { name } => println!("{:#?}", load_agent_template(root, &name)?),
        SkillsCommand::Hooks { event } => {
            let hooks = load_hooks(root)?;
            match event.as_deref().map(|name| hook_event(name).ok_or(name)) {
                Some(Ok(event)) => println!("{:#?}", hooks.commands(event)),
                Some(Err(name)) => bail!("unknown hook event `{name}`"),
                None => println!("{hooks:#?}"),
            }
        }
    }
    Ok(())
}

async fn mcp(command: McpCommand) -> Result<()> {
    match command {
        McpCommand::ListTools { program, args } => {
            let mut client = connect_mcp(program, args).await?;
            println!("{:#?}", client.list_tools().await?);
            client.shutdown().await?;
        }
        McpCommand::Call {
            program,
            tool,
            arguments,
            args,
        } => {
            let mut client = connect_mcp(program, args).await?;
            println!(
                "{}",
                client
                    .call_tool(&tool, serde_json::from_str(&arguments)?)
                    .await?
            );
            client.shutdown().await?;
        }
    }
    Ok(())
}

fn worktree(root: &Path, command: WorktreeCommand) -> Result<()> {
    let manager = GitWorktreeManager::new(root)?;
    match command {
        WorktreeCommand::Create { name, base } => {
            println!("{}", manager.create(&name, base.as_deref())?.path.display());
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
        WorktreeCommand::Status { name } => println!("{}", manager.status(&name)?),
        WorktreeCommand::Diff { name } => println!("{}", manager.diff(&name)?),
        WorktreeCommand::Review { name } => {
            let candidate = manager.review_candidate(&name)?;
            println!(
                "Review {} (branch {})\n\n## Status\n{}\n\n## Diff\n{}",
                candidate.workspace.name,
                candidate.workspace.branch,
                if candidate.status.is_empty() {
                    "(clean)"
                } else {
                    &candidate.status
                },
                if candidate.diff.is_empty() {
                    "(no uncommitted changes)"
                } else {
                    &candidate.diff
                }
            );
            println!("\nMerge only after review: nexus worktree merge {name} --approve");
        }
        WorktreeCommand::Merge { name, approve } => {
            manager.merge_after_review(&name, "HEAD", approve)?;
            println!("merged {name} after explicit approval");
        }
        WorktreeCommand::Remove { name, force } => manager.remove(&name, force)?,
        WorktreeCommand::AllocateAgents {
            run_name,
            count,
            base,
        } => {
            for workspace in manager.allocate_agents(&run_name, count, base.as_deref())? {
                println!(
                    "{}\t{}",
                    workspace.agent_index + 1,
                    workspace.worktree.path.display()
                );
            }
        }
    }
    Ok(())
}

async fn connect_mcp(program: String, args: Vec<String>) -> Result<StdioMcpClient> {
    let mut command = McpServerCommand::new(program);
    command.args = args;
    let mut client = StdioMcpClient::connect(&command)?;
    client.initialize("nexus").await?;
    Ok(client)
}

fn hook_event(name: &str) -> Option<HookEvent> {
    match name {
        "run_started" => Some(HookEvent::RunStarted),
        "before_model" => Some(HookEvent::BeforeModel),
        "before_tool" => Some(HookEvent::BeforeTool),
        "after_tool" => Some(HookEvent::AfterTool),
        "run_completed" => Some(HookEvent::RunCompleted),
        "run_failed" => Some(HookEvent::RunFailed),
        _ => None,
    }
}
