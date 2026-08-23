use std::{
    env,
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use nexus_agents::{AgentError, AgentJob, ParallelAgentScheduler};
use nexus_config::Config;
use nexus_context::{
    CodeIndex, ContextOptions, discover, discover_git_aware, discover_instructions,
};
use nexus_core::SessionId;
use nexus_mcp::{McpServerCommand, StdioMcpClient};
use nexus_models::{
    MockModelProvider, ModelProvider, ModelStreamEvent, OpenAiCompatibleProvider,
};
use nexus_permissions::{
    PermissionApprover, PermissionDecision, PermissionRequest, RuleBasedPolicy,
};
use nexus_runtime::{AgentRuntime, AuditEvent, AuditSink, AuthorizedToolExecutor};
use nexus_skills::{
    HookEvent, discover_skills, load_agent_template, load_hooks, load_skill,
};
use nexus_storage::{SessionStore, SqliteStore};
use nexus_tools::{FileSystemTool, ShellTool, ToolRegistry};
use nexus_workspace::{AgentWorkspace, GitWorktreeManager};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "nexus", version, about = "A configurable AI agent harness")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run {
        task: String,
        #[arg(short, long)]
        stream: bool,
        #[arg(short, long, default_value = "mock-1")]
        model: String,
        #[arg(long, default_value = "mock")]
        provider: String,
        #[arg(long, default_value = "https://api.openai.com/v1")]
        base_url: String,
        #[arg(long, default_value = "OPENAI_API_KEY")]
        api_key_env: String,
        #[arg(long)]
        tools: bool,
        #[arg(long)]
        max_steps: Option<usize>,
        #[arg(long)]
        yes: bool,
        /// Persist the complete execution trace in .nexus/nexus.db.
        #[arg(long)]
        session: bool,
        /// Prioritize Git-modified and untracked files in agent context guidance.
        #[arg(long)]
        git_context: bool,
        /// Reusable agent template from .nexus/agents/<name>.toml.
        #[arg(long)]
        agent_template: Option<String>,
        /// Explicit project-local skill to load. May be repeated.
        #[arg(long = "skill")]
        skills: Vec<String>,
    },
    Models,
    /// Display resolved configuration and configuration sources.
    Config,
    /// Inspect repository context and deterministic token budgets.
    Context {
        #[arg(long)]
        max_files: Option<usize>,
        #[arg(long)]
        token_budget: Option<usize>,
        #[arg(long)]
        git_aware: bool,
        #[arg(long)]
        model: Option<String>,
    },
    /// Search the lightweight repository code index.
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show the resolved hierarchical instruction chain.
    Instructions {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Inspect project-local skills, hooks, and agent templates.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
    /// Interact with an MCP server over stdio.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Execute multiple real Nexus runs in isolated worktrees.
    Agents {
        #[command(subcommand)]
        command: AgentsCommand,
    },
    /// Replay persisted execution events from a session ID.
    Replay { session_id: String },
    Doctor,
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
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
enum AgentsCommand {
    Run {
        task: String,
        count: usize,
        #[arg(long, default_value_t = 2)]
        concurrency: usize,
        #[arg(long)]
        base: Option<String>,
        #[arg(long, default_value = "mock-1")]
        model: String,
        #[arg(long, default_value = "mock")]
        provider: String,
        #[arg(long, default_value = "https://api.openai.com/v1")]
        base_url: String,
        #[arg(long, default_value = "OPENAI_API_KEY")]
        api_key_env: String,
        #[arg(long)]
        tools: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        max_steps: Option<usize>,
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
    Remove {
        name: String,
        #[arg(long)]
        force: bool,
    },
    /// Allocate one isolated worktree for every parallel agent in a run.
    AllocateAgents {
        run_name: String,
        count: usize,
        #[arg(long)]
        base: Option<String>,
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

struct SqliteAuditSink {
    store: Arc<SqliteStore>,
    session_id: String,
}

impl AuditSink for SqliteAuditSink {
    fn record(&self, event: AuditEvent) {
        if let Err(error) = self
            .store
            .append_event(&self.session_id, &event.kind, &event.payload)
        {
            eprintln!("[nexus] failed to persist audit event: {error}");
        }
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
                format!(
                    "missing API key environment variable `{api_key_env}` for the selected provider"
                )
            })?;
            Ok(Arc::new(OpenAiCompatibleProvider::new(
                "openai-compatible",
                base_url,
                api_key,
            )?))
        }
        _ => bail!(
            "unknown provider `{provider_name}`; supported providers: mock, openai-compatible"
        ),
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
    Ok(if yes {
        executor.with_approver(Arc::new(ApproveAll))
    } else {
        executor.with_approver(Arc::new(StdinApprover))
    })
}

fn session_store(root: &PathBuf) -> Result<Arc<SqliteStore>> {
    let directory = root.join(".nexus");
    fs::create_dir_all(&directory)?;
    Ok(Arc::new(SqliteStore::open(directory.join("nexus.db"))?))
}

fn resolved_instructions(
    root: &PathBuf,
    target: &PathBuf,
    agent_template: Option<&str>,
    skill_names: &[String],
    git_context: bool,
) -> Result<String> {
    let mut template_instructions = None;
    let mut selected_skills = skill_names.to_vec();
    if let Some(template_name) = agent_template {
        let template = load_agent_template(root, template_name)?;
        template_instructions = Some(template.instructions);
        selected_skills.extend(template.skills);
    }
    selected_skills.sort();
    selected_skills.dedup();
    let mut instruction_set = discover_instructions(root, target, template_instructions)?;
    for name in selected_skills {
        let skill = load_skill(root, &name)?;
        instruction_set.documents.push(nexus_context::InstructionDocument {
            path: skill.path.strip_prefix(root).unwrap_or(&skill.path).to_path_buf(),
            content: format!("# Skill: {}\n{}", skill.name, skill.instructions),
        });
    }
    let mut combined = instruction_set.combined();
    if git_context {
        let snapshot = discover_git_aware(root, &ContextOptions::default())?;
        let files = snapshot
            .prioritized_files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        if !files.is_empty() {
            combined.push_str("\n\n## Git-aware priority\nPrioritize these changed or untracked files when investigating the task:\n");
            combined.push_str(&files.join("\n"));
        }
    }
    Ok(combined)
}

struct RuntimeAgentJob {
    config: Config,
    model: String,
    provider: String,
    base_url: String,
    api_key_env: String,
    task: String,
    tools: bool,
    yes: bool,
    max_steps: usize,
}

#[async_trait]
impl AgentJob for RuntimeAgentJob {
    async fn run(&self, workspace: AgentWorkspace) -> Result<String, AgentError> {
        let provider = build_provider(&self.provider, &self.base_url, &self.api_key_env)
            .map_err(|error| AgentError::Execution(error.to_string()))?;
        let runtime = AgentRuntime::new(self.config.clone(), provider, self.model.clone());
        if !self.tools {
            let result = runtime
                .run(&self.task)
                .await
                .map_err(|error| AgentError::Execution(error.to_string()))?;
            return Ok(result.message);
        }
        let executor = build_tool_executor(workspace.worktree.path.clone(), self.yes)
            .map_err(|error| AgentError::Execution(error.to_string()))?;
        let instructions = resolved_instructions(
            &workspace.worktree.path,
            &workspace.worktree.path,
            None,
            &[],
            true,
        )
        .map_err(|error| AgentError::Execution(error.to_string()))?;
        let result = runtime
            .run_with_tools_controlled_with_instructions(
                &self.task,
                Some(&instructions),
                &executor,
                self.max_steps,
                CancellationToken::new(),
                None,
            )
            .await
            .map_err(|error| AgentError::Execution(error.to_string()))?;
        Ok(result.message)
    }
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();
    let cli = Cli::parse();
    let root = env::current_dir()?;
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
            session,
            git_context,
            agent_template,
            skills,
        }) => {
            if stream && tools {
                bail!("streaming with model-driven tool execution is not implemented yet");
            }
            let resolved = Config::load_from(&root)?;
            let step_limit = max_steps.unwrap_or(resolved.config.max_steps);
            let model_provider = build_provider(&provider, &base_url, &api_key_env)?;
            let runtime = AgentRuntime::new(resolved.config, model_provider, model.clone());
            let session_id = SessionId::new();
            let store = if session {
                let store = session_store(&root)?;
                store.create(session_id.clone()).await?;
                Some(store)
            } else {
                None
            };
            let audit = store.as_ref().map(|store| SqliteAuditSink {
                store: Arc::clone(store),
                session_id: session_id.0.to_string(),
            });

            let result = if stream {
                runtime
                    .run_streaming(&task, |event| match event {
                        ModelStreamEvent::Started { model } => {
                            eprintln!("[nexus] model: {}", model.0)
                        }
                        ModelStreamEvent::Delta { content } => print!("{content}"),
                        ModelStreamEvent::Completed { usage } => eprintln!(
                            "\n[nexus] usage: {} input / {} output tokens",
                            usage.input_tokens, usage.output_tokens
                        ),
                    })
                    .await?
            } else if tools {
                let executor = build_tool_executor(root.clone(), yes)?;
                let instructions = resolved_instructions(
                    &root,
                    &root,
                    agent_template.as_deref(),
                    &skills,
                    git_context,
                )?;
                let cancellation = CancellationToken::new();
                let signal_token = cancellation.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        eprintln!("[nexus] cancellation requested");
                        signal_token.cancel();
                    }
                });
                runtime
                    .run_with_tools_controlled_with_instructions(
                        &task,
                        Some(&instructions),
                        &executor,
                        step_limit,
                        cancellation,
                        audit.as_ref().map(|sink| sink as &dyn AuditSink),
                    )
                    .await?
            } else {
                if let Some(audit) = audit.as_ref() {
                    audit.record(AuditEvent {
                        kind: "run.started".to_owned(),
                        payload: serde_json::json!({"task": task, "provider": provider}),
                    });
                }
                let result = runtime.run(&task).await?;
                if let Some(audit) = audit.as_ref() {
                    audit.record(AuditEvent {
                        kind: "run.completed".to_owned(),
                        payload: serde_json::json!({
                            "task_id": result.task_id,
                            "message": result.message,
                            "provider": result.provider,
                            "model": result.model,
                            "input_tokens": result.usage.input_tokens,
                            "output_tokens": result.usage.output_tokens
                        }),
                    });
                }
                result
            };
            if session {
                eprintln!("[nexus] session {} persisted", session_id.0);
            }
            if !stream {
                println!("{}", result.message);
            }
            eprintln!(
                "[nexus] run {} completed via {} using {} input / {} output tokens",
                result.task_id,
                result.provider,
                result.usage.input_tokens,
                result.usage.output_tokens
            );
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
            println!(
                "context.token_budget = {}",
                resolved.config.context.token_budget
            );
            if resolved.sources.is_empty() {
                println!("sources = built-in defaults + environment");
            } else {
                for source in resolved.sources {
                    println!("source = {}", source.display());
                }
            }
        }
        Some(Command::Context {
            max_files,
            token_budget,
            git_aware,
            model,
        }) => {
            let resolved = Config::load_from(&root)?;
            let options = ContextOptions {
                max_files: max_files.unwrap_or(resolved.config.context.max_files),
                max_bytes_per_file: resolved.config.context.max_bytes_per_file,
                token_budget: token_budget.unwrap_or(resolved.config.context.token_budget),
            };
            if git_aware {
                let snapshot = discover_git_aware(&root, &options)?;
                println!("root = {}", snapshot.snapshot.root.display());
                println!("files = {}", snapshot.snapshot.files.len());
                println!(
                    "estimated_tokens = {}",
                    snapshot.snapshot.total_estimated_tokens
                );
                println!("git_available = {}", snapshot.git_available);
                println!("prioritized_files = {}", snapshot.prioritized_files.len());
                for file in snapshot.snapshot.files {
                    println!("{}\t{} tokens", file.path.display(), file.estimated_tokens);
                }
            } else {
                let snapshot = discover(&root, &options)?;
                println!("root = {}", snapshot.root.display());
                println!("files = {}", snapshot.files.len());
                println!("estimated_tokens = {}", snapshot.total_estimated_tokens);
                println!("truncated = {}", snapshot.truncated);
                if let Some(model) = model {
                    let estimated = snapshot
                        .files
                        .iter()
                        .map(|file| nexus_context::estimate_tokens_for_model(&file.content, &model))
                        .sum::<usize>();
                    println!("model_estimated_tokens = {estimated}");
                }
                for file in snapshot.files {
                    println!("{}\t{} tokens", file.path.display(), file.estimated_tokens);
                }
            }
        }
        Some(Command::Search { query, limit }) => {
            let snapshot = discover(&root, &ContextOptions::default())?;
            for entry in CodeIndex::build(&snapshot).search(&query, limit) {
                println!(
                    "{}:{}\t[score={}]\t{}",
                    entry.path.display(),
                    entry.line,
                    entry.score,
                    entry.text
                );
            }
        }
        Some(Command::Instructions { path }) => {
            let instructions = discover_instructions(&root, &path, None)?;
            for document in instructions.documents {
                println!("{}", document.path.display());
            }
        }
        Some(Command::Skills { command }) => match command {
            SkillsCommand::List => {
                for skill in discover_skills(&root)? {
                    println!("{}\t{}", skill.name, skill.path.display());
                }
            }
            SkillsCommand::Show { name } => {
                let skill = load_skill(&root, &name)?;
                println!("{}", skill.instructions);
            }
            SkillsCommand::Hooks { event } => {
                let hooks = load_hooks(&root)?;
                if let Some(event) = event {
                    let event = hook_event(&event).ok_or_else(|| {
                        anyhow::anyhow!("unknown hook event; use run_started, before_model, before_tool, after_tool, run_completed, or run_failed")
                    })?;
                    for command in hooks.commands(event) {
                        println!("{command}");
                    }
                } else {
                    for event in [
                        HookEvent::RunStarted,
                        HookEvent::BeforeModel,
                        HookEvent::BeforeTool,
                        HookEvent::AfterTool,
                        HookEvent::RunCompleted,
                        HookEvent::RunFailed,
                    ] {
                        println!(
                            "{}\t{}",
                            format!("{event:?}").to_ascii_lowercase(),
                            hooks.commands(event).len()
                        );
                    }
                }
            }
            SkillsCommand::Template { name } => {
                let template = load_agent_template(&root, &name)?;
                println!("name = {}", template.name);
                println!("description = {}", template.description);
                println!("skills = {}", template.skills.join(","));
                println!("\n{}", template.instructions);
            }
        },
        Some(Command::Mcp { command }) => match command {
            McpCommand::ListTools { program, args } => {
                let command = McpServerCommand {
                    program,
                    args,
                    env: Default::default(),
                };
                let mut client = StdioMcpClient::connect(&command).await?;
                let _ = client.initialize("nexus").await?;
                for tool in client.list_tools().await? {
                    println!("{}\t{}", tool.name, tool.description);
                }
                client.shutdown().await?;
            }
            McpCommand::Call {
                program,
                tool,
                arguments,
                args,
            } => {
                let command = McpServerCommand {
                    program,
                    args,
                    env: Default::default(),
                };
                let arguments = serde_json::from_str(&arguments)
                    .context("MCP tool arguments must be valid JSON")?;
                let mut client = StdioMcpClient::connect(&command).await?;
                let _ = client.initialize("nexus").await?;
                println!("{}", client.call_tool(&tool, arguments).await?);
                client.shutdown().await?;
            }
        },
        Some(Command::Agents { command }) => match command {
            AgentsCommand::Run {
                task,
                count,
                concurrency,
                base,
                model,
                provider,
                base_url,
                api_key_env,
                tools,
                yes,
                max_steps,
            } => {
                let manager = GitWorktreeManager::new(root.clone())?;
                let config = Config::load_from(&root)?.config;
                let job = RuntimeAgentJob {
                    max_steps: max_steps.unwrap_or(config.max_steps),
                    config,
                    model,
                    provider,
                    base_url,
                    api_key_env,
                    task,
                    tools,
                    yes,
                };
                let scheduler = ParallelAgentScheduler::new(concurrency);
                let run_name = format!("agents-{}", SessionId::new().0);
                let outcomes = scheduler
                    .execute(
                        &manager,
                        &run_name,
                        count,
                        base.as_deref(),
                        Arc::new(job),
                    )
                    .await?;
                for outcome in outcomes {
                    println!(
                        "agent-{}\t{}\t{}",
                        outcome.agent_index + 1,
                        outcome.workspace.worktree.path.display(),
                        outcome.summary
                    );
                }
            }
        },
        Some(Command::Replay { session_id }) => {
            let store = session_store(&root)?;
            for event in store.replay(&session_id)? {
                println!("{}\t{}\t{}", event.sequence, event.kind, event.payload);
            }
        }
        Some(Command::Doctor) => {
            println!("Nexus runtime: cancellation + bounded execution available");
            println!("Model gateway: mock + OpenAI-compatible adapter available");
            println!("Tool registry: filesystem + structured shell available");
            println!("Permission approvals: CLI boundary available");
            println!("Workspace isolation: Git worktrees + real parallel agent runs available");
            println!("Storage: SQLite audit sessions + replay available");
            println!("Context: Git-aware discovery + hierarchical instructions + code search available");
            println!("Extensibility: MCP stdio + local skills/hooks/templates available");
        }
        Some(Command::Worktree { command }) => {
            let manager = GitWorktreeManager::new(root)?;
            match command {
                WorktreeCommand::Create { name, base } => {
                    let worktree = manager.create(&name, base.as_deref())?;
                    println!(
                        "Created workspace `{}`\nBranch: {}\nPath: {}",
                        worktree.name,
                        worktree.branch,
                        worktree.path.display()
                    );
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
                WorktreeCommand::Remove { name, force } => {
                    manager.remove(&name, force)?;
                    println!("Removed workspace `{name}`. No changes were merged.");
                }
                WorktreeCommand::AllocateAgents {
                    run_name,
                    count,
                    base,
                } => {
                    for workspace in manager.allocate_agents(&run_name, count, base.as_deref())? {
                        println!(
                            "agent-{}\t{}\t{}",
                            workspace.agent_index + 1,
                            workspace.worktree.branch,
                            workspace.worktree.path.display()
                        );
                    }
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
