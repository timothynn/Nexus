//! Provider-neutral model contracts and a small provider registry.
//!
//! Nexus keeps these types independent from any vendor SDK. Concrete providers
//! translate their native request/response formats at the boundary.

use std::{collections::HashMap, pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_core::Stream;
use futures_util::stream;
use serde::{Deserialize, Serialize};

pub type ModelEventStream = Pin<Box<dyn Stream<Item = Result<ModelStreamEvent, ModelError>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(pub String);

impl From<&str> for ModelId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for ModelId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub chat: bool,
    pub streaming: bool,
    pub tool_calling: bool,
    pub structured_output: bool,
    pub vision: bool,
    pub reasoning: bool,
}

impl ModelCapabilities {
    #[must_use]
    pub const fn chat_only() -> Self {
        Self {
            chat: true,
            streaming: true,
            tool_calling: false,
            structured_output: false,
            vision: false,
            reasoning: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

impl ChatMessage {
    #[must_use]
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: ModelId,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
}

impl ModelRequest {
    #[must_use]
    pub fn from_prompt(model: impl Into<ModelId>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: vec![ChatMessage::new(MessageRole::User, prompt)],
            temperature: None,
            max_output_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    pub model: ModelId,
    pub message: ChatMessage,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStreamEvent {
    Started { model: ModelId },
    Delta { content: String },
    Completed { usage: Usage },
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;

    fn capabilities(&self) -> ModelCapabilities;

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;

    async fn stream(&self, request: ModelRequest) -> Result<ModelEventStream, ModelError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("model provider error: {0}")]
    Provider(String),
    #[error("model not found: {0}")]
    NotFound(String),
    #[error("unsupported capability: {0}")]
    Unsupported(String),
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn ModelProvider>>,
}

impl ProviderRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn ModelProvider>) -> Option<Arc<dyn ModelProvider>> {
        self.providers.insert(provider.name().to_owned(), provider)
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn ModelProvider>, ModelError> {
        self.providers
            .get(name)
            .cloned()
            .ok_or_else(|| ModelError::NotFound(name.to_owned()))
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut names = self.providers.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }
}

/// Deterministic provider used by the runtime and tests before real vendor
/// adapters are configured. It deliberately lives in the model crate so the
/// harness can be exercised without credentials or network access.
pub struct MockModelProvider {
    name: String,
}

impl Default for MockModelProvider {
    fn default() -> Self {
        Self::new("mock")
    }
}

impl MockModelProvider {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[async_trait]
impl ModelProvider for MockModelProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities::chat_only()
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let prompt = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map_or("", |message| message.content.as_str());

        Ok(ModelResponse {
            model: request.model,
            message: ChatMessage::new(MessageRole::Assistant, format!("Mock Nexus response: {prompt}")),
            usage: Usage {
                input_tokens: request.messages.len() as u64,
                output_tokens: 4,
            },
        })
    }

    async fn stream(&self, request: ModelRequest) -> Result<ModelEventStream, ModelError> {
        let response = self.complete(request).await?;
        let events = vec![
            Ok(ModelStreamEvent::Started {
                model: response.model,
            }),
            Ok(ModelStreamEvent::Delta {
                content: response.message.content,
            }),
            Ok(ModelStreamEvent::Completed {
                usage: response.usage,
            }),
        ];

        Ok(Box::pin(stream::iter(events)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{MockModelProvider, ModelProvider, ModelRequest, ProviderRegistry};

    #[tokio::test]
    async fn mock_provider_completes_a_prompt() {
        let provider = MockModelProvider::default();
        let response = provider
            .complete(ModelRequest::from_prompt("mock-1", "hello nexus"))
            .await
            .expect("mock provider should succeed");

        assert!(response.message.content.contains("hello nexus"));
    }

    #[test]
    fn registry_resolves_registered_provider() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(MockModelProvider::default()));

        assert_eq!(registry.names(), vec!["mock"]);
        assert_eq!(registry.get("mock").expect("provider should exist").name(), "mock");
    }
}
