//! Parallel, dependency-aware multi-agent orchestration over isolated workspaces.

use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

use async_trait::async_trait;
use futures_util::{stream, StreamExt};
use nexus_workspace::{AgentWorkspace, GitWorktreeManager, WorkspaceError};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct AgentOutcome { pub agent_index: usize, pub workspace: AgentWorkspace, pub summary: String }

#[derive(Debug, Clone)]
pub struct AgentHandoff { pub from: AgentRole, pub task_id: String, pub summary: String, pub workspace: AgentWorkspace }

#[async_trait]
pub trait AgentJob: Send + Sync { async fn run(&self, workspace: AgentWorkspace, cancellation: CancellationToken) -> Result<String, AgentError>; }

pub struct ParallelAgentScheduler { max_concurrency: usize }
impl ParallelAgentScheduler {
    #[must_use] pub fn new(max_concurrency: usize) -> Self { Self { max_concurrency } }
    pub async fn execute(&self, manager: &GitWorktreeManager, run_name: &str, count: usize, base: Option<&str>, job: Arc<dyn AgentJob>, cancellation: CancellationToken) -> Result<Vec<AgentOutcome>, AgentError> {
        if self.max_concurrency == 0 { return Err(AgentError::InvalidConcurrency); }
        let workspaces = manager.allocate_agents(run_name, count, base)?;
        let mut outcomes = stream::iter(workspaces.into_iter().map(|workspace| {
            let job = Arc::clone(&job); let cancellation = cancellation.child_token();
            async move {
                if cancellation.is_cancelled() { return Err(AgentError::Cancelled); }
                let agent_index = workspace.agent_index;
                let summary = tokio::select! {
                    _ = cancellation.cancelled() => Err(AgentError::Cancelled),
                    result = job.run(workspace.clone(), cancellation.child_token()) => result,
                }?;
                Ok::<_, AgentError>(AgentOutcome { agent_index, workspace, summary })
            }
        })).buffer_unordered(self.max_concurrency).collect::<Vec<_>>().await.into_iter().collect::<Result<Vec<_>, _>>()?;
        outcomes.sort_by_key(|outcome| outcome.agent_index);
        Ok(outcomes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskNode { pub id: String, pub depends_on: Vec<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskGraph { pub tasks: Vec<TaskNode> }
impl TaskGraph {
    pub fn layers(&self) -> Result<Vec<Vec<String>>, AgentError> {
        let mut remaining = self.tasks.iter().map(|task| (task.id.clone(), task.depends_on.iter().cloned().collect::<BTreeSet<_>>())).collect::<BTreeMap<_, _>>();
        let known = remaining.keys().cloned().collect::<BTreeSet<_>>();
        if let Some((task, dependency)) = remaining.iter().find_map(|(task, deps)| deps.iter().find(|dep| !known.contains(*dep)).map(|dep| (task.clone(), dep.clone()))) { return Err(AgentError::UnknownDependency { task, dependency }); }
        let mut layers = Vec::new();
        while !remaining.is_empty() {
            let ready = remaining.iter().filter(|(_, deps)| deps.is_empty()).map(|(id, _)| id.clone()).collect::<Vec<_>>();
            if ready.is_empty() { return Err(AgentError::CyclicTaskGraph); }
            for id in &ready { remaining.remove(id); }
            let completed = ready.iter().cloned().collect::<BTreeSet<_>>();
            for deps in remaining.values_mut() { deps.retain(|dep| !completed.contains(dep)); }
            layers.push(ready);
        }
        Ok(layers)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole { Worker, Supervisor, Reviewer }
#[derive(Debug, Clone)]
pub struct AgentPlan { pub role: AgentRole, pub task_id: String, pub instructions: String }

pub fn build_supervisor_review_plan(tasks: &[TaskNode], supervisor_instructions: &str, reviewer_instructions: &str) -> Vec<AgentPlan> {
    let mut plans = tasks.iter().map(|task| AgentPlan { role: AgentRole::Worker, task_id: task.id.clone(), instructions: format!("Execute task `{}` independently.", task.id) }).collect::<Vec<_>>();
    plans.push(AgentPlan { role: AgentRole::Supervisor, task_id: "supervisor".to_owned(), instructions: supervisor_instructions.to_owned() });
    plans.push(AgentPlan { role: AgentRole::Reviewer, task_id: "reviewer".to_owned(), instructions: reviewer_instructions.to_owned() });
    plans
}

#[async_trait]
pub trait RoleRunner: Send + Sync {
    async fn run(&self, plan: AgentPlan, handoffs: Vec<AgentHandoff>, cancellation: CancellationToken) -> Result<String, AgentError>;
}

#[derive(Debug, Clone)]
pub struct OrchestrationResult { pub workers: Vec<AgentHandoff>, pub supervisor: AgentHandoff, pub reviewer: AgentHandoff }

pub struct MultiAgentCoordinator { max_concurrency: usize }
impl MultiAgentCoordinator {
    #[must_use] pub fn new(max_concurrency: usize) -> Self { Self { max_concurrency } }
    pub async fn execute_graph(&self, graph: &TaskGraph, manager: &GitWorktreeManager, run_name: &str, base: Option<&str>, worker: Arc<dyn AgentJob>, supervisor: Arc<dyn RoleRunner>, reviewer: Arc<dyn RoleRunner>, cancellation: CancellationToken) -> Result<OrchestrationResult, AgentError> {
        let layers = graph.layers()?;
        let scheduler = ParallelAgentScheduler::new(self.max_concurrency);
        let mut handoffs = Vec::new();
        for layer in layers {
            if cancellation.is_cancelled() { return Err(AgentError::Cancelled); }
            let layer_name = format!("{run_name}-{}", layer.first().cloned().unwrap_or_else(|| "layer".to_owned()));
            let outcomes = scheduler.execute(manager, &layer_name, layer.len(), base, Arc::clone(&worker), cancellation.child_token()).await?;
            for (task_id, outcome) in layer.into_iter().zip(outcomes) { handoffs.push(AgentHandoff { from: AgentRole::Worker, task_id, summary: outcome.summary, workspace: outcome.workspace }); }
        }
        let supervisor_workspace = handoffs.first().map(|handoff| handoff.workspace.clone()).ok_or(AgentError::EmptyGraph)?;
        let supervisor_plan = AgentPlan { role: AgentRole::Supervisor, task_id: "supervisor".to_owned(), instructions: "Aggregate worker handoffs, resolve conflicts, and produce an implementation plan.".to_owned() };
        let supervisor_summary = supervisor.run(supervisor_plan, handoffs.clone(), cancellation.child_token()).await?;
        let supervisor_handoff = AgentHandoff { from: AgentRole::Supervisor, task_id: "supervisor".to_owned(), summary: supervisor_summary, workspace: supervisor_workspace.clone() };
        let reviewer_plan = AgentPlan { role: AgentRole::Reviewer, task_id: "reviewer".to_owned(), instructions: "Review worker and supervisor output for correctness, regressions, and unmet requirements. Do not merge changes.".to_owned() };
        let reviewer_summary = reviewer.run(reviewer_plan, [handoffs.clone(), vec![supervisor_handoff.clone()]].concat(), cancellation.child_token()).await?;
        let reviewer_handoff = AgentHandoff { from: AgentRole::Reviewer, task_id: "reviewer".to_owned(), summary: reviewer_summary, workspace: supervisor_workspace };
        Ok(OrchestrationResult { workers: handoffs, supervisor: supervisor_handoff, reviewer: reviewer_handoff })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)] Workspace(#[from] WorkspaceError),
    #[error("parallel agent concurrency must be greater than zero")] InvalidConcurrency,
    #[error("agent execution was cancelled")] Cancelled,
    #[error("task graph contains no tasks")] EmptyGraph,
    #[error("agent execution failed: {0}")] Execution(String),
    #[error("task `{task}` depends on unknown task `{dependency}`")] UnknownDependency { task: String, dependency: String },
    #[error("task graph contains a dependency cycle")] CyclicTaskGraph,
}

#[cfg(test)]
mod tests {
    use super::{TaskGraph, TaskNode};
    #[test]
    fn graph_builds_dependency_layers() {
        let graph = TaskGraph { tasks: vec![TaskNode { id: "a".into(), depends_on: vec![] }, TaskNode { id: "b".into(), depends_on: vec!["a".into()] }] };
        assert_eq!(graph.layers().expect("valid graph"), vec![vec!["a".to_owned()], vec!["b".to_owned()]]);
    }
}
