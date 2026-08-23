//! Parallel, dependency-aware agent orchestration over isolated workspaces.

use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

use async_trait::async_trait;
use futures_util::{stream, StreamExt};
use nexus_workspace::{AgentWorkspace, GitWorktreeManager, WorkspaceError};

#[derive(Debug, Clone)]
pub struct AgentOutcome { pub agent_index: usize, pub workspace: AgentWorkspace, pub summary: String }

#[async_trait]
pub trait AgentJob: Send + Sync { async fn run(&self, workspace: AgentWorkspace) -> Result<String, AgentError>; }

pub struct ParallelAgentScheduler { max_concurrency: usize }
impl ParallelAgentScheduler {
    #[must_use] pub fn new(max_concurrency: usize) -> Self { Self { max_concurrency } }
    pub async fn execute(&self, manager: &GitWorktreeManager, run_name: &str, count: usize, base: Option<&str>, job: Arc<dyn AgentJob>) -> Result<Vec<AgentOutcome>, AgentError> {
        if self.max_concurrency == 0 { return Err(AgentError::InvalidConcurrency); }
        let workspaces = manager.allocate_agents(run_name, count, base)?;
        let mut outcomes = stream::iter(workspaces.into_iter().map(|workspace| {
            let job = Arc::clone(&job);
            async move { let agent_index = workspace.agent_index; let summary = job.run(workspace.clone()).await?; Ok::<_, AgentError>(AgentOutcome { agent_index, workspace, summary }) }
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

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)] Workspace(#[from] WorkspaceError),
    #[error("parallel agent concurrency must be greater than zero")] InvalidConcurrency,
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
