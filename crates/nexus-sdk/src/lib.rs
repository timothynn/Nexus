//! Public embedding API for Nexus.
//!
//! The SDK intentionally exposes a small, stable facade over the runtime so
//! applications can embed Nexus without depending directly on every internal
//! crate. Advanced integrations can still opt into lower-level crates.

use std::sync::Arc;

use nexus_config::Config;
use nexus_models::{ModelId, ModelProvider};
use nexus_runtime::{AgentRuntime, RunResult, RuntimeError};
use tokio_util::sync::CancellationToken;

/// A configured Nexus instance suitable for embedding in another application.
pub struct Nexus {
    runtime: AgentRuntime,
}

impl Nexus {
    /// Starts building an embedded Nexus instance.
    #[must_use]
    pub fn builder() -> NexusBuilder {
        NexusBuilder::default()
    }

    /// Executes a single prompt through the configured model provider.
    pub async fn run(&self, request: RunRequest) -> Result<RunResult, SdkError> {
        if request.cancellation.is_cancelled() {
            return Err(SdkError::Cancelled);
        }

        tokio::select! {
            _ = request.cancellation.cancelled() => Err(SdkError::Cancelled),
            result = self.runtime.run(&request.prompt) => result.map_err(SdkError::Runtime),
        }
    }

    /// Returns the configured runtime for advanced integrations.
    #[must_use]
    pub fn runtime(&self) -> &AgentRuntime {
        &self.runtime
    }
}

/// Builder for [`Nexus`].
pub struct NexusBuilder {
    config: Config,
    provider: Option<Arc<dyn ModelProvider>>,
    model: Option<ModelId>,
}

impl Default for NexusBuilder {
    fn default() -> Self {
        Self {
            config: Config::default(),
            provider: None,
            model: None,
        }
    }
}

impl NexusBuilder {
    #[must_use]
    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    #[must_use]
    pub fn provider(mut self, provider: Arc<dyn ModelProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    #[must_use]
    pub fn model(mut self, model: impl Into<ModelId>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn build(self) -> Result<Nexus, SdkError> {
        let provider = self.provider.ok_or(SdkError::MissingProvider)?;
        let model = self.model.ok_or(SdkError::MissingModel)?;
        Ok(Nexus {
            runtime: AgentRuntime::new(self.config, provider, model),
        })
    }
}

/// A cancellable SDK execution request.
#[derive(Clone)]
pub struct RunRequest {
    pub prompt: String,
    pub cancellation: CancellationToken,
}

impl RunRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            cancellation: CancellationToken::new(),
        }
    }

    #[must_use]
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("a model provider must be configured")]
    MissingProvider,
    #[error("a model must be configured")]
    MissingModel,
    #[error("Nexus execution was cancelled")]
    Cancelled,
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nexus_models::MockModelProvider;

    use super::{Nexus, RunRequest};

    #[tokio::test]
    async fn sdk_executes_with_a_configured_provider() {
        let nexus = Nexus::builder()
            .provider(Arc::new(MockModelProvider::default()))
            .model("mock-1")
            .build()
            .expect("builder should be complete");

        let result = nexus
            .run(RunRequest::new("summarize this repository"))
            .await
            .expect("run should succeed");

        assert!(result.message.contains("summarize this repository"));
    }
}
