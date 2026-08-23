//! Agent execution runtime.
//!
//! The runtime coordinates stable domain contracts with provider-neutral model
//! adapters. Tools and permissions will join this loop in the next milestone.

use std::sync::Arc;

use futures_util::StreamExt;
use nexus_config::Config;
use nexus_core::Task;
use nexus_models::{
    ModelError, ModelId, ModelProvider, ModelRequest, ModelStreamEvent, Usage,
};

#[derive(Debug, Clone)]
pub struct RunResult {
    pub task_id: String,
    pub provider: String,
    pub model: String,
    pub message: String,
    pub usage: Usage,
}

pub struct AgentRuntime {
    config: Config,
    provider: Arc<dyn ModelProvider>,
    model: ModelId,
}

impl AgentRuntime {
    #[must_use]
    pub fn new(
        config: Config,
        provider: Arc<dyn ModelProvider>,
        model: impl Into<ModelId>,
    ) -> Self {
        Self {
            config,
            provider,
            model: model.into(),
        }
    }

    pub async fn run(&self, prompt: &str) -> Result<RunResult, RuntimeError> {
        let task = Task::new(prompt);
        let request = ModelRequest::from_prompt(self.model.clone(), task.prompt.clone());
        let response = self.provider.complete(request).await?;

        Ok(RunResult {
            task_id: task.id.to_string(),
            provider: self.provider.name().to_owned(),
            model: response.model.0,
            message: response.message.content,
            usage: response.usage,
        })
    }

    pub async fn run_streaming<F>(
        &self,
        prompt: &str,
        mut on_event: F,
    ) -> Result<RunResult, RuntimeError>
    where
        F: FnMut(&ModelStreamEvent),
    {
        let task = Task::new(prompt);
        let request = ModelRequest::from_prompt(self.model.clone(), task.prompt.clone());
        let mut stream = self.provider.stream(request).await?;

        let mut message = String::new();
        let mut usage = Usage::default();

        while let Some(event) = stream.next().await {
            let event = event?;
            match &event {
                ModelStreamEvent::Delta { content } => message.push_str(content),
                ModelStreamEvent::Completed {
                    usage: completed_usage,
                } => {
                    usage = completed_usage.clone();
                }
                ModelStreamEvent::Started { .. } => {}
            }
            on_event(&event);
        }

        Ok(RunResult {
            task_id: task.id.to_string(),
            provider: self.provider.name().to_owned(),
            model: self.model.0.clone(),
            message,
            usage,
        })
    }

    #[must_use]
    pub fn default_agent(&self) -> &str {
        &self.config.default_agent
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Model(#[from] ModelError),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nexus_config::Config;
    use nexus_models::{MockModelProvider, ModelStreamEvent};

    use super::AgentRuntime;

    #[tokio::test]
    async fn runtime_executes_with_mock_provider() {
        let runtime = AgentRuntime::new(
            Config::default(),
            Arc::new(MockModelProvider::default()),
            "mock-1",
        );

        let result = runtime
            .run("explain the architecture")
            .await
            .expect("run should succeed");

        assert_eq!(result.provider, "mock");
        assert!(result.message.contains("explain the architecture"));
    }

    #[tokio::test]
    async fn runtime_streams_events() {
        let runtime = AgentRuntime::new(
            Config::default(),
            Arc::new(MockModelProvider::default()),
            "mock-1",
        );
        let mut saw_delta = false;

        let result = runtime
            .run_streaming("stream this", |event| {
                if matches!(event, ModelStreamEvent::Delta { .. }) {
                    saw_delta = true;
                }
            })
            .await
            .expect("stream should succeed");

        assert!(saw_delta);
        assert!(result.message.contains("stream this"));
    }
}
