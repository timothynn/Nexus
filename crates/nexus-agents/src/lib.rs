//! Parallel, dependency-aware multi-agent orchestration over isolated workspaces.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use async_trait::async_trait;
use futures_util::{stream, StreamExt};
use nexus_workspace::{AgentWorkspace, GitWorktreeManager, WorkspaceError};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub agent_index: usize,
    pub workspace: AgentWorkspace,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct AgentHandoff {
    pub from: AgentRole,
    pub task_id: String,
    pub summary: String,
    pub workspace: AgentWorkspace,
}

/// Structured, run-level observability events emitted by schedulers and coordinators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    RunStarted { run_name: String },
    LayerStarted { tasks: Vec<String> },
    WorkerStarted { task_id: String, agent_index: usize, workspace: String },
    WorkerCompleted { task_id: String, agent_index: usize },
    WorkerFailed { task_id: String, agent_index: usize, error: String },
    RoleStarted { role: AgentRole, task_id: String },
    RoleCompleted { role: AgentRole, task_id: String },
    RunCompleted { run_name: String },
    RunCancelled { run_name: String },
}

pub trait AgentEventSink: Send + Sync {
    fn record(&self, event: AgentEvent);
}

fn record_event(sink: Option<&dyn AgentEventSink>, event: AgentEvent) {
    if let Some(sink) = sink {
        sink.record(event);
    }
}

#[async_trait]
pub trait AgentJob: Send + Sync {
    async fn run(
        &self,
        workspace: AgentWorkspace,
        cancellation: CancellationToken,
    ) -> Result<String, AgentError>;
}

pub struct ParallelAgentScheduler {
    max_concurrency: usize,
    events: Option<Arc<dyn AgentEventSink>>,
}

impl ParallelAgentScheduler {
    #[must_use]
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency,
            events: None,
        }
    }

    #[must_use]
    pub fn with_event_sink(mut self, events: Arc<dyn AgentEventSink>) -> Self {
        self.events = Some(events);
        self
    }

    pub async fn execute(
        &self,
        manager: &GitWorktreeManager,
        run_name: &str,
        count: usize,
        base: Option<&str>,
        job: Arc<dyn AgentJob>,
        cancellation: CancellationToken,
    ) -> Result<Vec<AgentOutcome>, AgentError> {
        if self.max_concurrency == 0 {
            return Err(AgentError::InvalidConcurrency);
        }
        record_event(
            self.events.as_deref(),
            AgentEvent::RunStarted {
                run_name: run_name.to_owned(),
            },
        );
        let workspaces = manager.allocate_agents(run_name, count, base)?;
        let outcomes = stream::iter(workspaces.into_iter().map(|workspace| {
            let job = Arc::clone(&job);
            let cancellation = cancellation.child_token();
            let events = self.events.clone();
            async move {
                if cancellation.is_cancelled() {
                    return Err(AgentError::Cancelled);
                }
                let agent_index = workspace.agent_index;
                let workspace_name = workspace.worktree.name.clone();
                record_event(
                    events.as_deref(),
                    AgentEvent::WorkerStarted {
                        task_id: format!("agent-{}", agent_index + 1),
                        agent_index,
                        workspace: workspace_name,
                    },
                );
                let summary = tokio::select! {
                    _ = cancellation.cancelled() => Err(AgentError::Cancelled),
                    result = job.run(workspace.clone(), cancellation.child_token()) => result,
                };
                match summary {
                    Ok(summary) => {
                        record_event(
                            events.as_deref(),
                            AgentEvent::WorkerCompleted {
                                task_id: format!("agent-{}", agent_index + 1),
                                agent_index,
                            },
                        );
                        Ok(AgentOutcome { agent_index, workspace, summary })
                    }
                    Err(error) => {
                        record_event(
                            events.as_deref(),
                            AgentEvent::WorkerFailed {
                                task_id: format!("agent-{}", agent_index + 1),
                                agent_index,
                                error: error.to_string(),
                            },
                        );
                        Err(error)
                    }
                }
            }
        }))
        .buffer_unordered(self.max_concurrency)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>();

        match outcomes {
            Ok(mut outcomes) => {
                outcomes.sort_by_key(|outcome| outcome.agent_index);
                record_event(
                    self.events.as_deref(),
                    AgentEvent::RunCompleted {
                        run_name: run_name.to_owned(),
                    },
                );
                Ok(outcomes)
            }
            Err(AgentError::Cancelled) => {
                record_event(
                    self.events.as_deref(),
                    AgentEvent::RunCancelled {
                        run_name: run_name.to_owned(),
                    },
                );
                Err(AgentError::Cancelled)
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskNode {
    pub id: String,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskGraph {
    pub tasks: Vec<TaskNode>,
}

impl TaskGraph {
    pub fn layers(&self) -> Result<Vec<Vec<String>>, AgentError> {
        let mut remaining = self
            .tasks
            .iter()
            .map(|task| {
                (
                    task.id.clone(),
                    task.depends_on.iter().cloned().collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let known = remaining.keys().cloned().collect::<BTreeSet<_>>();
        if let Some((task, dependency)) = remaining.iter().find_map(|(task, deps)| {
            deps.iter()
                .find(|dep| !known.contains(*dep))
                .map(|dep| (task.clone(), dep.clone()))
        }) {
            return Err(AgentError::UnknownDependency { task, dependency });
        }
        let mut layers = Vec::new();
        while !remaining.is_empty() {
            let ready = remaining
                .iter()
                .filter(|(_, deps)| deps.is_empty())
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if ready.is_empty() {
                return Err(AgentError::CyclicTaskGraph);
            }
            for id in &ready {
                remaining.remove(id);
            }
            let completed = ready.iter().cloned().collect::<BTreeSet<_>>();
            for deps in remaining.values_mut() {
                deps.retain(|dep| !completed.contains(dep));
            }
            layers.push(ready);
        }
        Ok(layers)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Worker,
    Supervisor,
    Reviewer,
}

#[derive(Debug, Clone)]
pub struct AgentPlan {
    pub role: AgentRole,
    pub task_id: String,
    pub instructions: String,
}

pub fn build_supervisor_review_plan(
    tasks: &[TaskNode],
    supervisor_instructions: &str,
    reviewer_instructions: &str,
) -> Vec<AgentPlan> {
    let mut plans = tasks
        .iter()
        .map(|task| AgentPlan {
            role: AgentRole::Worker,
            task_id: task.id.clone(),
            instructions: format!("Execute task `{}` independently.", task.id),
        })
        .collect::<Vec<_>>();
    plans.push(AgentPlan {
        role: AgentRole::Supervisor,
        task_id: "supervisor".to_owned(),
        instructions: supervisor_instructions.to_owned(),
    });
    plans.push(AgentPlan {
        role: AgentRole::Reviewer,
        task_id: "reviewer".to_owned(),
        instructions: reviewer_instructions.to_owned(),
    });
    plans
}

#[async_trait]
pub trait RoleRunner: Send + Sync {
    async fn run(
        &self,
        plan: AgentPlan,
        handoffs: Vec<AgentHandoff>,
        cancellation: CancellationToken,
    ) -> Result<String, AgentError>;
}

#[derive(Debug, Clone)]
pub struct OrchestrationResult {
    pub workers: Vec<AgentHandoff>,
    pub supervisor: AgentHandoff,
    pub reviewer: AgentHandoff,
}

pub struct MultiAgentCoordinator {
    max_concurrency: usize,
    events: Option<Arc<dyn AgentEventSink>>,
}

impl MultiAgentCoordinator {
    #[must_use]
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency,
            events: None,
        }
    }

    #[must_use]
    pub fn with_event_sink(mut self, events: Arc<dyn AgentEventSink>) -> Self {
        self.events = Some(events);
        self
    }

    pub async fn execute_graph(
        &self,
        graph: &TaskGraph,
        manager: &GitWorktreeManager,
        run_name: &str,
        base: Option<&str>,
        worker: Arc<dyn AgentJob>,
        supervisor: Arc<dyn RoleRunner>,
        reviewer: Arc<dyn RoleRunner>,
        cancellation: CancellationToken,
    ) -> Result<OrchestrationResult, AgentError> {
        let layers = graph.layers()?;
        let mut handoffs = Vec::new();
        record_event(
            self.events.as_deref(),
            AgentEvent::RunStarted {
                run_name: run_name.to_owned(),
            },
        );

        for layer in layers {
            if cancellation.is_cancelled() {
                record_event(
                    self.events.as_deref(),
                    AgentEvent::RunCancelled {
                        run_name: run_name.to_owned(),
                    },
                );
                return Err(AgentError::Cancelled);
            }
            record_event(
                self.events.as_deref(),
                AgentEvent::LayerStarted {
                    tasks: layer.clone(),
                },
            );
            let layer_name = format!(
                "{run_name}-{}",
                layer.first().cloned().unwrap_or_else(|| "layer".to_owned())
            );
            let scheduler = if let Some(events) = &self.events {
                ParallelAgentScheduler::new(self.max_concurrency).with_event_sink(Arc::clone(events))
            } else {
                ParallelAgentScheduler::new(self.max_concurrency)
            };
            let outcomes = scheduler
                .execute(
                    manager,
                    &layer_name,
                    layer.len(),
                    base,
                    Arc::clone(&worker),
                    cancellation.child_token(),
                )
                .await?;
            for (task_id, outcome) in layer.into_iter().zip(outcomes) {
                handoffs.push(AgentHandoff {
                    from: AgentRole::Worker,
                    task_id,
                    summary: outcome.summary,
                    workspace: outcome.workspace,
                });
            }
        }

        let supervisor_workspace = handoffs
            .first()
            .map(|handoff| handoff.workspace.clone())
            .ok_or(AgentError::EmptyGraph)?;
        let supervisor_plan = AgentPlan {
            role: AgentRole::Supervisor,
            task_id: "supervisor".to_owned(),
            instructions: "Aggregate worker handoffs, resolve conflicts, and produce an implementation plan."
                .to_owned(),
        };
        record_event(
            self.events.as_deref(),
            AgentEvent::RoleStarted {
                role: AgentRole::Supervisor,
                task_id: supervisor_plan.task_id.clone(),
            },
        );
        let supervisor_summary = supervisor
            .run(
                supervisor_plan,
                handoffs.clone(),
                cancellation.child_token(),
            )
            .await?;
        record_event(
            self.events.as_deref(),
            AgentEvent::RoleCompleted {
                role: AgentRole::Supervisor,
                task_id: "supervisor".to_owned(),
            },
        );
        let supervisor_handoff = AgentHandoff {
            from: AgentRole::Supervisor,
            task_id: "supervisor".to_owned(),
            summary: supervisor_summary,
            workspace: supervisor_workspace.clone(),
        };

        let reviewer_plan = AgentPlan {
            role: AgentRole::Reviewer,
            task_id: "reviewer".to_owned(),
            instructions: "Review worker and supervisor output for correctness, regressions, and unmet requirements. Do not merge changes."
                .to_owned(),
        };
        record_event(
            self.events.as_deref(),
            AgentEvent::RoleStarted {
                role: AgentRole::Reviewer,
                task_id: reviewer_plan.task_id.clone(),
            },
        );
        let reviewer_summary = reviewer
            .run(
                reviewer_plan,
                [handoffs.clone(), vec![supervisor_handoff.clone()]].concat(),
                cancellation.child_token(),
            )
            .await?;
        record_event(
            self.events.as_deref(),
            AgentEvent::RoleCompleted {
                role: AgentRole::Reviewer,
                task_id: "reviewer".to_owned(),
            },
        );
        let reviewer_handoff = AgentHandoff {
            from: AgentRole::Reviewer,
            task_id: "reviewer".to_owned(),
            summary: reviewer_summary,
            workspace: supervisor_workspace,
        };
        record_event(
            self.events.as_deref(),
            AgentEvent::RunCompleted {
                run_name: run_name.to_owned(),
            },
        );
        Ok(OrchestrationResult {
            workers: handoffs,
            supervisor: supervisor_handoff,
            reviewer: reviewer_handoff,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("parallel agent concurrency must be greater than zero")]
    InvalidConcurrency,
    #[error("agent execution was cancelled")]
    Cancelled,
    #[error("task graph contains no tasks")]
    EmptyGraph,
    #[error("agent execution failed: {0}")]
    Execution(String),
    #[error("task `{task}` depends on unknown task `{dependency}`")]
    UnknownDependency { task: String, dependency: String },
    #[error("task graph contains a dependency cycle")]
    CyclicTaskGraph,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{AgentEvent, AgentEventSink, TaskGraph, TaskNode};

    struct RecordingSink(Mutex<Vec<AgentEvent>>);
    impl AgentEventSink for RecordingSink {
        fn record(&self, event: AgentEvent) {
            self.0.lock().expect("sink lock").push(event);
        }
    }

    #[test]
    fn graph_builds_dependency_layers() {
        let graph = TaskGraph {
            tasks: vec![
                TaskNode {
                    id: "a".into(),
                    depends_on: vec![],
                },
                TaskNode {
                    id: "b".into(),
                    depends_on: vec!["a".into()],
                },
            ],
        };
        assert_eq!(
            graph.layers().expect("valid graph"),
            vec![vec!["a".to_owned()], vec!["b".to_owned()]]
        );
    }

    #[test]
    fn recording_sink_is_shareable() {
        let sink = Arc::new(RecordingSink(Mutex::new(Vec::new())));
        sink.record(AgentEvent::RunStarted { run_name: "test".into() });
        assert_eq!(sink.0.lock().expect("sink lock").len(), 1);
    }
}
