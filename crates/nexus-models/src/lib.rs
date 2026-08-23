//! Provider-neutral model contracts and model provider adapters.
//!
//! Nexus keeps domain types independent from vendor SDKs. Concrete providers
//! translate native request/response formats at the boundary.

use std::{collections::HashMap, pin::Pin, sync::Arc};

use async_trait::async_trait;
use futures_core::Stream;
use futures_util::stream;
use reqwest::Client;
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
            chat: true,
            streaming: false,
            tool_calling: true,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ModelToolCall>,
}

impl ChatMessage {
    #[must_use]
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    #[must_use]
    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ModelToolCall>,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls,
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
            tool_calls: Vec::new(),
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

/// Provider for services that expose the OpenAI-compatible Chat Completions API.
///
/// This adapter is intentionally configured with a base URL instead of being
/// coupled to one vendor. The same Nexus model/tool contracts can therefore be
/// translated for self-hosted or third-party compatible endpoints.
pub struct OpenAiCompatibleProvider {
    name: String,
    endpoint: String,
    api_key: String,
    client: Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let name = name.into();
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        let api_key = api_key.into();
        if name.is_empty() || base_url.is_empty() || api_key.is_empty() {
            return Err(ModelError::Provider(
                "provider name, base URL, and API key must not be empty".to_owned(),
            ));
        }

        Ok(Self {
            name,
            endpoint: format!("{base_url}/chat/completions"),
            api_key,
            client: Client::new(),
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities::with_tool_calling()
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let body = OpenAiCompatibleRequest::from(&request);
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| ModelError::Provider(error.to_string()))?
            .error_for_status()
            .map_err(|error| ModelError::Provider(error.to_string()))?
            .json::<OpenAiCompatibleResponse>()
            .await
            .map_err(|error| ModelError::Provider(error.to_string()))?;

        response.into_model_response(request.model)
    }

    async fn stream(&self, _request: ModelRequest) -> Result<ModelEventStream, ModelError> {
        Err(ModelError::Unsupported(
            "streaming for the OpenAI-compatible provider is not implemented yet".to_owned(),
        ))
    }
}

#[derive(Debug, Serialize)]
struct OpenAiCompatibleRequest {
    model: String,
    messages: Vec<OpenAiCompatibleMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiCompatibleTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

impl From<&ModelRequest> for OpenAiCompatibleRequest {
    fn from(request: &ModelRequest) -> Self {
        Self {
            model: request.model.0.clone(),
            messages: request
                .messages
                .iter()
                .map(OpenAiCompatibleMessage::from)
                .collect(),
            tools: request
                .tools
                .iter()
                .map(OpenAiCompatibleTool::from)
                .collect(),
            temperature: request.temperature,
            max_tokens: request.max_output_tokens,
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiCompatibleMessage {
    role: &'static str,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OpenAiCompatibleToolCall>,
}

impl From<&ChatMessage> for OpenAiCompatibleMessage {
    fn from(message: &ChatMessage) -> Self {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        Self {
            role,
            content: message.content.clone(),
            name: message.name.clone(),
            tool_call_id: message.tool_call_id.clone(),
            tool_calls: message
                .tool_calls
                .iter()
                .map(OpenAiCompatibleToolCall::from)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiCompatibleTool {
    r#type: &'static str,
    function: OpenAiCompatibleFunctionDefinition,
}

impl From<&ModelToolDefinition> for OpenAiCompatibleTool {
    fn from(tool: &ModelToolDefinition) -> Self {
        Self {
            r#type: "function",
            function: OpenAiCompatibleFunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiCompatibleFunctionDefinition {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct OpenAiCompatibleToolCall {
    id: String,
    r#type: &'static str,
    function: OpenAiCompatibleFunctionCall,
}

impl From<&ModelToolCall> for OpenAiCompatibleToolCall {
    fn from(call: &ModelToolCall) -> Self {
        Self {
            id: call.id.clone(),
            r#type: "function",
            function: OpenAiCompatibleFunctionCall {
                name: call.name.clone(),
                arguments: call.arguments.to_string(),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiCompatibleFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleResponse {
    choices: Vec<OpenAiCompatibleChoice>,
    #[serde(default)]
    usage: OpenAiCompatibleUsage,
}

impl OpenAiCompatibleResponse {
    fn into_model_response(self, fallback_model: ModelId) -> Result<ModelResponse, ModelError> {
        let choice = self
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ModelError::Provider("provider returned no choices".to_owned()))?;
        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(OpenAiCompatibleToolCallResponse::into_model_tool_call)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ModelResponse {
            model: fallback_model,
            message: ChatMessage::assistant_with_tool_calls(
                choice.message.content.unwrap_or_default(),
                tool_calls.clone(),
            ),
            tool_calls,
            usage: Usage {
                input_tokens: self.usage.prompt_tokens,
                output_tokens: self.usage.completion_tokens,
            },
        })
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleChoice {
    message: OpenAiCompatibleResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiCompatibleToolCallResponse>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleToolCallResponse {
    id: String,
    function: OpenAiCompatibleFunctionCall,
}

impl OpenAiCompatibleToolCallResponse {
    fn into_model_tool_call(self) -> Result<ModelToolCall, ModelError> {
        let arguments = serde_json::from_str(&self.function.arguments).map_err(|error| {
            ModelError::Provider(format!("invalid tool call arguments from provider: {error}"))
        })?;
        Ok(ModelToolCall {
            id: self.id,
            name: self.function.name,
            arguments,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiCompatibleUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{json, to_value};

    use super::{
        ChatMessage, MessageRole, MockModelProvider, ModelProvider, ModelRequest, ModelToolCall,
        ModelToolDefinition, OpenAiCompatibleProvider, OpenAiCompatibleRequest,
        OpenAiCompatibleResponse, ProviderRegistry,
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

    #[test]
    fn openai_compatible_request_preserves_tool_history() {
        let call = ModelToolCall {
            id: "call-1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({ "value": 42 }),
        };
        let request = ModelRequest {
            model: "example-model".into(),
            messages: vec![
                ChatMessage::new(MessageRole::User, "hello"),
                ChatMessage::assistant_with_tool_calls("", vec![call.clone()]),
                ChatMessage::tool("echo", "call-1", "{\"value\":42}"),
            ],
            tools: vec![ModelToolDefinition {
                name: "echo".to_owned(),
                description: "echoes input".to_owned(),
                input_schema: json!({ "type": "object" }),
            }],
            temperature: None,
            max_output_tokens: None,
        };

        let value = to_value(OpenAiCompatibleRequest::from(&request))
            .expect("request should serialize");

        assert_eq!(value["tools"][0]["function"]["name"], "echo");
        assert_eq!(
            value["messages"][1]["tool_calls"][0]["function"]["arguments"],
            "{\"value\":42}"
        );
        assert_eq!(value["messages"][2]["tool_call_id"], "call-1");
    }

    #[test]
    fn openai_compatible_response_extracts_tool_calls() {
        let response: OpenAiCompatibleResponse = serde_json::from_value(json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "function": {
                            "name": "echo",
                            "arguments": "{\"value\":42}"
                        }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 2
            }
        }))
        .expect("response fixture should deserialize");

        let result = response
            .into_model_response("example-model".into())
            .expect("response should map");

        assert_eq!(result.tool_calls[0].name, "echo");
        assert_eq!(result.message.tool_calls[0].id, "call-1");
        assert_eq!(result.usage.input_tokens, 10);
    }

    #[test]
    fn openai_compatible_provider_builds_normalized_endpoint() {
        let provider = OpenAiCompatibleProvider::new(
            "compatible",
            "https://example.com/v1/",
            "test-key",
        )
        .expect("provider should be valid");

        assert_eq!(provider.endpoint(), "https://example.com/v1/chat/completions");
    }
}
