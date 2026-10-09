//! `nexus agents`: parallel workers and dependency-aware worker → supervisor → reviewer graphs.

use std::{path::Path, sync::Arc};

use anyhow::{Result, bail};
use async_trait::async_trait;
use clap::{Args, Subcommand};
use nexus_agents::{
    AgentError, AgentHandoff, AgentJob, AgentPlan, GraphRunners, MultiAgentCoordinator,
    OrchestrationResult, ParallelAgentScheduler, RoleRunner, TaskGraph, TaskNode,
};
use nexus_config::Config;
use nexus_core::SessionId;
use nexus_runtime::AgentRuntime;
use nexus_workspace::{AgentWorkspace, GitWorktreeManager};
use tokio_util::sync::CancellationToken;

use crate::support::{
    Approval, ProviderArgs, WORKSPACE_TOOLS, build_tool_executor, resolved_instructions,
};

#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
    /// Run the same task in N isolated worktrees concurrently.
    Run {
        task: String,
        count: usize,
        #[command(flatten)]
        options: AgentOptions,
    },
    /// Execute a dependency-aware worker → supervisor → reviewer graph.
    Graph {
        /// Task IDs, optionally with dependencies: `implement:research` `tests:implement`
        tasks: Vec<String>,
        #[command(flatten)]
        options: AgentOptions,
    },
}

#[derive(Debug, Args)]
pub struct AgentOptions {
    #[arg(long, default_value_t = 2)]
    concurrency: usize,
    #[arg(long)]
    base: Option<String>,
    #[command(flatten)]
    provider: ProviderArgs,
    #[arg(long)]
    tools: bool,
    #[arg(long)]
    yes: bool,
    #[arg(long)]
    max_steps: Option<usize>,
}

pub async fn run(root: &Path, command: AgentsCommand) -> Result<()> {
    let resolved = Config::load_from(root)?;
    let manager = GitWorktreeManager::new(root)?;
    match command {
        AgentsCommand::Run {
            task,
            count,
            options,
        } => {
            let run_name = format!("agents-{}", SessionId::new().0);
            let job = Arc::new(RuntimeAgentJob::new(resolved.config, &options, task));
            let outcomes = ParallelAgentScheduler::new(options.concurrency)
                .execute(
                    &manager,
                    &run_name,
                    count,
                    options.base.as_deref(),
                    job,
                    CancellationToken::new(),
                )
                .await?;
            for outcome in outcomes {
                println!(
                    "agent {} [{}]: {}",
                    outcome.agent_index + 1,
                    outcome.workspace.worktree.name,
                    outcome.summary
                );
            }
        }
        AgentsCommand::Graph { tasks, options } => {
            let graph = parse_graph_tasks(&tasks)?;
            let run_name = format!("graph-{}", SessionId::new().0);
            let worker = Arc::new(RuntimeAgentJob::new(
                resolved.config.clone(),
                &options,
                "Execute the assigned graph task in this isolated workspace and return a concise handoff."
                    .to_owned(),
            ));
            let role_runner: Arc<dyn RoleRunner> = Arc::new(RuntimeRoleRunner {
                config: resolved.config,
                provider: options.provider.clone(),
            });
            let result = MultiAgentCoordinator::new(options.concurrency)
                .execute_graph(
                    &graph,
                    &manager,
                    &run_name,
                    options.base.as_deref(),
                    GraphRunners {
                        worker,
                        supervisor: Arc::clone(&role_runner),
                        reviewer: role_runner,
                    },
                    CancellationToken::new(),
                )
                .await?;
            print_orchestration(result);
        }
    }
    Ok(())
}

struct RuntimeAgentJob {
    config: Config,
    provider: ProviderArgs,
    task: String,
    tools: bool,
    approval: Approval,
    max_steps: usize,
}

impl RuntimeAgentJob {
    fn new(config: Config, options: &AgentOptions, task: String) -> Self {
        Self {
            max_steps: options.max_steps.unwrap_or(config.max_steps),
            config,
            provider: options.provider.clone(),
            task,
            tools: options.tools,
            approval: if options.yes {
                Approval::Always
            } else {
                Approval::Prompt
            },
        }
    }
}

#[allow(clippy::needless_pass_by_value)] // Used directly as a `map_err` adapter.
fn execution(error: impl ToString) -> AgentError {
    AgentError::Execution(error.to_string())
}

#[async_trait]
impl AgentJob for RuntimeAgentJob {
    async fn run(
        &self,
        workspace: AgentWorkspace,
        cancellation: CancellationToken,
    ) -> Result<String, AgentError> {
        let provider = self.provider.build().map_err(execution)?;
        let runtime = AgentRuntime::new(self.config.clone(), provider, self.provider.model.clone());
        if !self.tools {
            return runtime
                .run(&self.task)
                .await
                .map(|result| result.message)
                .map_err(execution);
        }
        let path = &workspace.worktree.path;
        let executor =
            build_tool_executor(path, &WORKSPACE_TOOLS, self.approval).map_err(execution)?;
        let instructions = resolved_instructions(path, path, None, &[], true).map_err(execution)?;
        runtime
            .run_with_tools_controlled_with_instructions(
                &self.task,
                Some(&instructions),
                &executor,
                self.max_steps,
                cancellation,
                None,
            )
            .await
            .map(|result| result.message)
            .map_err(execution)
    }
}

struct RuntimeRoleRunner {
    config: Config,
    provider: ProviderArgs,
}

#[async_trait]
impl RoleRunner for RuntimeRoleRunner {
    async fn run(
        &self,
        plan: AgentPlan,
        handoffs: Vec<AgentHandoff>,
        cancellation: CancellationToken,
    ) -> Result<String, AgentError> {
        if cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        let provider = self.provider.build().map_err(execution)?;
        let runtime = AgentRuntime::new(self.config.clone(), provider, self.provider.model.clone());
        let context = handoffs
            .iter()
            .map(|handoff| {
                format!(
                    "## {:?}: {}\n{}",
                    handoff.from, handoff.task_id, handoff.summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let prompt = format!(
            "{}\n\n# Role\n{:?}\n# Task\n{}\n\n# Handoffs\n{}",
            plan.instructions, plan.role, plan.task_id, context
        );
        runtime
            .run(&prompt)
            .await
            .map(|result| result.message)
            .map_err(execution)
    }
}

fn parse_graph_tasks(tasks: &[String]) -> Result<TaskGraph> {
    if tasks.is_empty() {
        bail!("provide at least one graph task");
    }
    let nodes = tasks
        .iter()
        .map(|spec| {
            let (id, dependencies) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
            let id = id.trim();
            if id.is_empty() {
                bail!("graph task IDs cannot be empty");
            }
            let depends_on = dependencies
                .split(',')
                .map(str::trim)
                .filter(|dependency| !dependency.is_empty())
                .map(str::to_owned)
                .collect();
            Ok(TaskNode {
                id: id.to_owned(),
                depends_on,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(TaskGraph { tasks: nodes })
}

fn print_orchestration(result: OrchestrationResult) {
    println!("workers:");
    for handoff in result.workers {
        println!(
            "  - {} [{}]: {}",
            handoff.task_id, handoff.workspace.worktree.name, handoff.summary
        );
    }
    println!("supervisor: {}", result.supervisor.summary);
    println!("reviewer: {}", result.reviewer.summary);
}

#[cfg(test)]
mod tests {
    use super::parse_graph_tasks;

    #[test]
    fn graph_specs_parse_dependencies() {
        let graph =
            parse_graph_tasks(&["research".to_owned(), "build:research, design".to_owned()])
                .expect("graph");
        assert_eq!(graph.tasks[1].depends_on, vec!["research", "design"]);
        assert!(parse_graph_tasks(&[]).is_err());
        assert!(parse_graph_tasks(&[":x".to_owned()]).is_err());
    }
}
