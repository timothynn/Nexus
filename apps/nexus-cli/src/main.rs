use std::{
    env,
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use nexus_agents::{
    AgentError, AgentHandoff, AgentJob, AgentPlan, AgentRole, MultiAgentCoordinator,
    OrchestrationResult, ParallelAgentScheduler, RoleRunner, TaskGraph, TaskNode,
};
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
        #[arg(long)]
        session: bool,
        #[arg(long)]
        git_context: bool,
        #[arg(long)]
        agent_template: Option<String>,
        #[arg(long = "skill")]
        skills: Vec<String>,
    },
    Models,
    Config,
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
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
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
    Agents {
        #[command(subcommand)]
        command: AgentsCommand,
    },
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
    /// Execute a dependency-aware worker → supervisor → reviewer graph.
    Graph {
        /// Task IDs, optionally with dependencies: implement:research,tests:implement
        tasks: Vec<String>,
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
    Create { name: String, #[arg(long)] base: Option<String> },
    List,
    Status { name: String },
    Diff { name: String },
    Remove { name: String, #[arg(long)] force: bool },
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

fn session_store(root: &PathBuf) -> Result<Arc<SqliteStore>> {
    let directory = root.join(".nexus");
    fs::create_dir_all(&directory)?;
    Ok(Arc::new(SqliteStore::open(directory.join("nexus.db"))?))
}

fn resolved_instructions(root: &PathBuf, target: &PathBuf, agent_template: Option<&str>, skill_names: &[String], git_context: bool) -> Result<String> {
    let mut template_instructions = None;
    let mut selected_skills = skill_names.to_vec();
    if let Some(template_name) = agent_template {
        let template = load_agent_template(root, template_name)?;
        template_instructions = Some(template.instructions);
        selected_skills.extend(template.skills);
    }
    selected_skills.sort(); selected_skills.dedup();
    let mut instruction_set = discover_instructions(root, target, template_instructions)?;
    for name in selected_skills {
        let skill = load_skill(root, &name)?;
        instruction_set.documents.push(nexus_context::InstructionDocument { path: skill.path.strip_prefix(root).unwrap_or(&skill.path).to_path_buf(), content: format!("# Skill: {}\n{}", skill.name, skill.instructions) });
    }
    let mut combined = instruction_set.combined();
    if git_context {
        let snapshot = discover_git_aware(root, &ContextOptions::default())?;
        let files = snapshot.prioritized_files.iter().map(|path| path.display().to_string()).collect::<Vec<_>>();
        if !files.is_empty() { combined.push_str("\n\n## Git-aware priority\nPrioritize these changed or untracked files when investigating the task:\n"); combined.push_str(&files.join("\n")); }
    }
    Ok(combined)
}

struct RuntimeAgentJob {
    config: Config, model: String, provider: String, base_url: String, api_key_env: String,
    task: String, tools: bool, yes: bool, max_steps: usize,
}

#[async_trait]
impl AgentJob for RuntimeAgentJob {
    async fn run(&self, workspace: AgentWorkspace, cancellation: CancellationToken) -> Result<String, AgentError> {
        let provider = build_provider(&self.provider, &self.base_url, &self.api_key_env).map_err(|error| AgentError::Execution(error.to_string()))?;
        let runtime = AgentRuntime::new(self.config.clone(), provider, self.model.clone());
        if !self.tools {
            return runtime.run(&self.task).await.map(|result| result.message).map_err(|error| AgentError::Execution(error.to_string()));
        }
        let executor = build_tool_executor(workspace.worktree.path.clone(), self.yes).map_err(|error| AgentError::Execution(error.to_string()))?;
        let instructions = resolved_instructions(&workspace.worktree.path, &workspace.worktree.path, None, &[], true).map_err(|error| AgentError::Execution(error.to_string()))?;
        runtime.run_with_tools_controlled_with_instructions(&self.task, Some(&instructions), &executor, self.max_steps, cancellation, None).await.map(|result| result.message).map_err(|error| AgentError::Execution(error.to_string()))
    }
}

struct RuntimeRoleRunner { config: Config, model: String, provider: String, base_url: String, api_key_env: String }
#[async_trait]
impl RoleRunner for RuntimeRoleRunner {
    async fn run(&self, plan: AgentPlan, handoffs: Vec<AgentHandoff>, cancellation: CancellationToken) -> Result<String, AgentError> {
        if cancellation.is_cancelled() { return Err(AgentError::Cancelled); }
        let provider = build_provider(&self.provider, &self.base_url, &self.api_key_env).map_err(|error| AgentError::Execution(error.to_string()))?;
        let runtime = AgentRuntime::new(self.config.clone(), provider, self.model.clone());
        let context = handoffs.iter().map(|handoff| format!("## {:?}: {}\n{}", handoff.from, handoff.task_id, handoff.summary)).collect::<Vec<_>>().join("\n\n");
        let prompt = format!("{}\n\n# Role\n{:?}\n# Task\n{}\n\n# Handoffs\n{}", plan.instructions, plan.role, plan.task_id, context);
        runtime.run(&prompt).await.map(|result| result.message).map_err(|error| AgentError::Execution(error.to_string()))
    }
}

fn parse_graph_tasks(tasks: &[String]) -> Result<TaskGraph> {
    if tasks.is_empty() { bail!("provide at least one graph task"); }
    let nodes = tasks.iter().map(|spec| {
        let (id, dependencies) = spec.split_once(':').map_or((spec.as_str(), ""), |(id, dependencies)| (id, dependencies));
        let id = id.trim();
        if id.is_empty() { bail!("graph task IDs cannot be empty"); }
        let depends_on = dependencies.split(',').filter_map(|dependency| { let dependency = dependency.trim(); (!dependency.is_empty()).then(|| dependency.to_owned()) }).collect();
        Ok(TaskNode { id: id.to_owned(), depends_on })
    }).collect::<Result<Vec<_>>>()?;
    Ok(TaskGraph { tasks: nodes })
}

fn print_orchestration(result: OrchestrationResult) {
    println!("workers:");
    for handoff in result.workers { println!("  - {} [{}]: {}", handoff.task_id, handoff.workspace.worktree.name, handoff.summary); }
    println!("supervisor: {}", result.supervisor.summary);
    println!("reviewer: {}", result.reviewer.summary);
}

fn hook_event(name: &str) -> Option<HookEvent> {
    match name { "run_started" => Some(HookEvent::RunStarted), "before_model" => Some(HookEvent::BeforeModel), "before_tool" => Some(HookEvent::BeforeTool), "after_tool" => Some(HookEvent::AfterTool), "run_completed" => Some(HookEvent::RunCompleted), "run_failed" => Some(HookEvent::RunFailed), _ => None }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).with_target(false).init();
    let cli = Cli::parse(); let root = env::current_dir()?;
    match cli.command {
        Some(Command::Agents { command: AgentsCommand::Graph { tasks, concurrency, base, model, provider, base_url, api_key_env, tools, yes, max_steps } }) => {
            let resolved = Config::load_from(&root)?;
            let max_steps = max_steps.unwrap_or(resolved.config.max_steps);
            let graph = parse_graph_tasks(&tasks)?;
            let manager = GitWorktreeManager::new(&root)?;
            let run_name = format!("graph-{}", SessionId::new().0);
            let worker = Arc::new(RuntimeAgentJob { config: resolved.config.clone(), model: model.clone(), provider: provider.clone(), base_url: base_url.clone(), api_key_env: api_key_env.clone(), task: "Execute the assigned graph task in this isolated workspace and return a concise handoff.".to_owned(), tools, yes, max_steps });
            let role_runner = Arc::new(RuntimeRoleRunner { config: resolved.config, model, provider, base_url, api_key_env });
            let cancellation = CancellationToken::new();
            let coordinator = MultiAgentCoordinator::new(concurrency);
            let result = coordinator.execute_graph(&graph, &manager, &run_name, base.as_deref(), worker, Arc::clone(&role_runner), role_runner, cancellation).await?;
            print_orchestration(result);
        }
        Some(Command::Agents { command: AgentsCommand::Run { task, count, concurrency, base, model, provider, base_url, api_key_env, tools, yes, max_steps } }) => {
            let resolved = Config::load_from(&root)?;
            let max_steps = max_steps.unwrap_or(resolved.config.max_steps);
            let manager = GitWorktreeManager::new(&root)?;
            let run_name = format!("agents-{}", SessionId::new().0);
            let job = Arc::new(RuntimeAgentJob { config: resolved.config, model, provider, base_url, api_key_env, task, tools, yes, max_steps });
            let outcomes = ParallelAgentScheduler::new(concurrency).execute(&manager, &run_name, count, base.as_deref(), job, CancellationToken::new()).await?;
            for outcome in outcomes { println!("agent {} [{}]: {}", outcome.agent_index + 1, outcome.workspace.worktree.name, outcome.summary); }
        }
        Some(Command::Run { task, stream, model, provider, base_url, api_key_env, tools, max_steps, yes, session: _, git_context, agent_template, skills }) => {
            if stream && tools { bail!("streaming with model-driven tool execution is not implemented yet"); }
            let resolved = Config::load_from(&root)?; let step_limit = max_steps.unwrap_or(resolved.config.max_steps);
            let model_provider = build_provider(&provider, &base_url, &api_key_env)?; let runtime = AgentRuntime::new(resolved.config, model_provider, model);
            if tools {
                let executor = build_tool_executor(root.clone(), yes)?;
                let instructions = resolved_instructions(&root, &root, agent_template.as_deref(), &skills, git_context)?;
                let result = runtime.run_with_tools_controlled_with_instructions(&task, Some(&instructions), &executor, step_limit, CancellationToken::new(), None).await?;
                println!("{}", result.message);
            } else if stream {
                let result = runtime.run_streaming(&task, |event| { if let ModelStreamEvent::Delta { content } = event { print!("{content}"); } }).await?; println!(); eprintln!("\n[usage] input={} output={} total={}", result.usage.input_tokens, result.usage.output_tokens, result.usage.total_tokens);
            } else { println!("{}", runtime.run(&task).await?.message); }
        }
        Some(Command::Models) => println!("mock\nopenai-compatible"),
        Some(Command::Config) => { let resolved = Config::load_from(&root)?; println!("{:#?}", resolved.config); }
        Some(Command::Context { max_files, token_budget, git_aware, model: _ }) => { let mut options = ContextOptions::default(); if let Some(max_files) = max_files { options.max_files = max_files; } if let Some(token_budget) = token_budget { options.token_budget = token_budget; } let snapshot = if git_aware { discover_git_aware(&root, &options)? } else { discover(&root, &options)? }; println!("files={} estimated_tokens={} truncated={}", snapshot.files.len(), snapshot.estimated_tokens, snapshot.truncated); }
        Some(Command::Search { query, limit }) => { let index = CodeIndex::build(&root, &ContextOptions::default())?; for result in index.search(&query, limit) { println!("{}:{}", result.path.display(), result.score); } }
        Some(Command::Instructions { path }) => println!("{}", discover_instructions(&root, &path, None)?.combined()),
        Some(Command::Skills { command: SkillsCommand::List }) => for skill in discover_skills(&root)? { println!("{}\t{}", skill.name, skill.description); },
        Some(Command::Skills { command: SkillsCommand::Show { name } }) => { let skill = load_skill(&root, &name)?; println!("{}", skill.instructions); }
        Some(Command::Skills { command: SkillsCommand::Template { name } }) => println!("{:#?}", load_agent_template(&root, &name)?),
        Some(Command::Skills { command: SkillsCommand::Hooks { event } }) => { let hooks = load_hooks(&root)?; if let Some(event) = event.as_deref().and_then(hook_event) { println!("{:#?}", hooks.for_event(event)); } else { println!("{:#?}", hooks); } }
        Some(Command::Mcp { command: McpCommand::ListTools { program, args } }) => { let mut client = StdioMcpClient::spawn(McpServerCommand { program, args }).await?; println!("{:#?}", client.list_tools().await?); }
        Some(Command::Mcp { command: McpCommand::Call { program, tool, arguments, args } }) => { let mut client = StdioMcpClient::spawn(McpServerCommand { program, args }).await?; println!("{}", client.call_tool(&tool, serde_json::from_str(&arguments)?).await?); }
        Some(Command::Replay { session_id }) => { let store = session_store(&root)?; for event in store.replay(&session_id).await? { println!("{:?}", event); } }
        Some(Command::Doctor) => println!("nexus doctor: workspace={}", root.display()),
        Some(Command::Worktree { command }) => { let manager = GitWorktreeManager::new(&root)?; match command { WorktreeCommand::Create { name, base } => println!("{}", manager.create(&name, base.as_deref())?.path.display()), WorktreeCommand::List => for worktree in manager.list()? { println!("{}\t{}\t{}", worktree.name, worktree.branch, worktree.path.display()); }, WorktreeCommand::Status { name } => println!("{}", manager.status(&name)?), WorktreeCommand::Diff { name } => println!("{}", manager.diff(&name)?), WorktreeCommand::Remove { name, force } => manager.remove(&name, force)?, WorktreeCommand::AllocateAgents { run_name, count, base } => for workspace in manager.allocate_agents(&run_name, count, base.as_deref())? { println!("{}\t{}", workspace.agent_index + 1, workspace.worktree.path.display()); } } }
        None => println!("Nexus: use `nexus --help` to inspect available commands."),
    }
    Ok(())
}
