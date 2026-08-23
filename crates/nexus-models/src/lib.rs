//! Provider-neutral model contracts and a small provider registry.
//!
//! Nexus keeps these types independent from any vendor SDK. Concrete providers
//! translate their native request/response formats at the boundary.

use std::{collections::HashMap, pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_core::Stream;
use futures_util::stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type ModelEventStream =
    Pin<Box<dyn Stream<Item = Result<ModelStreamEvent, ModelError>> + Send>>;

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

    #[must_use]
    pub const fn with_tool_calling() -> Self {
        Self {
            tool_calling: true,
            ..Self::chat_only()
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    #[must_use]
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            name: None,
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn tool(
        name: impl Into<String>,
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            name: Some(name.into()),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(default = "default_object_schema")]
    pub input_schema: Value,
}

fn default_object_schema() -> Value {
    serde_json::json!({ "type": "object" })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    #[serde(default = "default_object_value")]
    pub arguments: Value,
}

fn default_object_value() -> Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: ModelId,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tools: Vec<ModelToolDefinition>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
}

impl ModelRequest {
    #[must_use]
    pub fn from_prompt(model: impl Into<ModelId>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: vec![ChatMessage::new(MessageRole::User, prompt)],
            tools: Vec::new(),
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
    #[serde(default)]
    pub tool_calls: Vec<ModelToolCall>,
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
            message: ChatMessage::new(
                MessageRole::Assistant,
                format!("Mock Nexus response: {prompt}"),
            ),
            tool_calls: Vec::new(),
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

    use serde_json::json;

    use super::{
        ChatMessage, MessageRole, MockModelProvider, ModelProvider, ModelRequest, ModelToolCall,
        ModelToolDefinition, ProviderRegistry,
    };

    #[tokio::test]
    async fn mock_provider_completes_a_prompt() {
        let provider = MockModelProvider::default();
        let response = provider
            .complete(ModelRequest::from_prompt("mock-1", "hello nexus"))
            .await
            .expect("mock provider should succeed");

        assert!(response.message.content.contains("hello nexus"));
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn registry_resolves_registered_provider() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(MockModelProvider::default()));

        assert_eq!(registry.names(), vec!["mock"]);
        assert_eq!(
            registry
                .get("mock")
                .expect("provider should exist")
                .name(),
            "mock"
        );
    }

    #[test]
    fn tool_messages_and_calls_preserve_provider_neutral_metadata() {
        let message = ChatMessage::tool("echo", "call-1", "done");
        let call = ModelToolCall {
            id: "call-1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({ "value": 42 }),
        };
        let definition = ModelToolDefinition {
            name: "echo".to_owned(),
            description: "echoes input".to_owned(),
            input_schema: json!({ "type": "object" }),
        };

        assert_eq!(message.role, MessageRole::Tool);
        assert_eq!(message.name.as_deref(), Some("echo"));
        assert_eq!(call.arguments["value"], 42);
        assert_eq!(definition.input_schema["type"], "object");
    }
}
