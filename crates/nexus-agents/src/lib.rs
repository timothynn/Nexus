//! Parallel agent orchestration over isolated Nexus workspaces.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use nexus_workspace::{AgentWorkspace, GitWorktreeManager, WorkspaceError};

#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub agent_index: usize,
    pub workspace: AgentWorkspace,
    pub summary: String,
}

#[async_trait]
pub trait AgentJob: Send + Sync {
    async fn run(&self, workspace: AgentWorkspace) -> Result<String, AgentError>;
}

pub struct ParallelAgentScheduler {
    max_concurrency: usize,
}

impl ParallelAgentScheduler {
    #[must_use]
    pub fn new(max_concurrency: usize) -> Self { Self { max_concurrency } }

    pub async fn execute(
        &self,
        manager: &GitWorktreeManager,
        run_name: &str,
        count: usize,
        base: Option<&str>,
        job: Arc<dyn AgentJob>,
    ) -> Result<Vec<AgentOutcome>, AgentError> {
        if self.max_concurrency == 0 { return Err(AgentError::InvalidConcurrency); }
        let workspaces = manager.allocate_agents(run_name, count, base)?;
        let mut outcomes = stream::iter(workspaces.into_iter().map(|workspace| {
            let job = Arc::clone(&job);
            async move {
                let agent_index = workspace.agent_index;
                let summary = job.run(workspace.clone()).await?;
                Ok::<_, AgentError>(AgentOutcome { agent_index, workspace, summary })
            }
        })).buffer_unordered(self.max_concurrency).collect::<Vec<_>>().await.into_iter().collect::<Result<Vec<_>, _>>()?;
        outcomes.sort_by_key(|outcome| outcome.agent_index);
        Ok(outcomes)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)] Workspace(#[from] WorkspaceError),
    #[error("parallel agent concurrency must be greater than zero")] InvalidConcurrency,
    #[error("agent execution failed: {0}")] Execution(String),
}
