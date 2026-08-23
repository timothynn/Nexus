//! Agent execution runtime.
//!
//! The runtime coordinates stable domain contracts with provider-neutral model
//! adapters. Tool execution is kept behind explicit permission checks so
//! model-driven agent loops cannot bypass the execution policy.

use std::sync::Arc;

use futures_util::StreamExt;
use nexus_config::Config;
use nexus_core::Task;
use nexus_models::{
    ChatMessage, ModelError, ModelId, ModelProvider, ModelRequest, ModelResponse,
    ModelStreamEvent, ModelToolDefinition, Usage,
};
use nexus_permissions::{enforce, PermissionError, PermissionPolicy, PermissionRequest};
use nexus_tools::{ToolError, ToolRegistry, ToolRequest, ToolResponse};

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

        Ok(self.result_from_response(task.id.to_string(), response))
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

    /// Run a model-driven tool loop until the provider returns a final response
    /// or the configured step limit is reached.
    pub async fn run_with_tools(
        &self,
        prompt: &str,
        executor: &AuthorizedToolExecutor,
        max_steps: usize,
    ) -> Result<RunResult, RuntimeError> {
        if max_steps == 0 {
            return Err(RuntimeError::InvalidMaxSteps);
        }

        let tools = executor.model_tools();
        if !tools.is_empty() && !self.provider.capabilities().tool_calling {
            return Err(RuntimeError::Model(ModelError::Unsupported(
                "tool_calling".to_owned(),
            )));
        }

        let task = Task::new(prompt);
        let mut messages = vec![ChatMessage::new(
            nexus_models::MessageRole::User,
            task.prompt.clone(),
        )];
        let mut usage = Usage::default();

        for _step in 0..max_steps {
            let request = ModelRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
                temperature: None,
                max_output_tokens: None,
            };
            let response = self.provider.complete(request).await?;
            add_usage(&mut usage, &response.usage);

            if response.tool_calls.is_empty() {
                let mut result = self.result_from_response(task.id.to_string(), response);
                result.usage = usage;
                return Ok(result);
            }

            messages.push(response.message);
            for call in response.tool_calls {
                let output = executor
                    .execute(
                        &call.name,
                        ToolRequest {
                            input: call.arguments,
                        },
                    )
                    .await?;
                let content = serde_json::to_string(&output.output)?;
                messages.push(ChatMessage::tool(call.name, call.id, content));
            }
        }

        Err(RuntimeError::MaxToolSteps(max_steps))
    }

    #[must_use]
    pub fn default_agent(&self) -> &str {
        &self.config.default_agent
    }

    fn result_from_response(&self, task_id: String, response: ModelResponse) -> RunResult {
        RunResult {
            task_id,
            provider: self.provider.name().to_owned(),
            model: response.model.0,
            message: response.message.content,
            usage: response.usage,
        }
    }
}

fn add_usage(total: &mut Usage, usage: &Usage) {
    total.input_tokens += usage.input_tokens;
    total.output_tokens += usage.output_tokens;
}

/// Executes registered tools only after an explicit policy decision.
pub struct AuthorizedToolExecutor {
    registry: ToolRegistry,
    policy: Arc<dyn PermissionPolicy>,
}

impl AuthorizedToolExecutor {
    #[must_use]
    pub fn new(registry: ToolRegistry, policy: Arc<dyn PermissionPolicy>) -> Self {
        Self { registry, policy }
    }

    #[must_use]
    pub fn model_tools(&self) -> Vec<ModelToolDefinition> {
        self.registry
            .metadata()
            .into_iter()
            .map(|metadata| ModelToolDefinition {
                name: metadata.name,
                description: metadata.description,
                input_schema: metadata.input_schema,
            })
            .collect()
    }

    pub async fn execute(
        &self,
        tool_name: &str,
        request: ToolRequest,
    ) -> Result<ToolResponse, RuntimeError> {
        let permission_request = PermissionRequest::new(tool_name);
        enforce(self.policy.as_ref(), &permission_request)?;
        Ok(self.registry.execute(tool_name, request).await?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Permission(#[from] PermissionError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("max tool steps must be greater than zero")]
    InvalidMaxSteps,
    #[error("agent run exceeded the maximum of {0} tool steps")]
    MaxToolSteps(usize),
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Arc};

    use async_trait::async_trait;
    use futures_util::stream;
    use nexus_config::Config;
    use nexus_models::{
        ChatMessage, MockModelProvider, ModelCapabilities, ModelError, ModelEventStream, ModelId,
        ModelProvider, ModelRequest, ModelResponse, ModelStreamEvent, ModelToolCall, Usage,
    };
    use nexus_permissions::{PermissionDecision, RuleBasedPolicy};
    use nexus_tools::{Tool, ToolMetadata, ToolRegistry, ToolRequest, ToolResponse};
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::{AgentRuntime, AuthorizedToolExecutor, RuntimeError};

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn metadata(&self) -> ToolMetadata {
            ToolMetadata {
                name: "echo".to_owned(),
                description: "echoes input".to_owned(),
                input_schema: json!({ "type": "object" }),
            }
        }

        async fn execute(
            &self,
            request: ToolRequest,
        ) -> Result<ToolResponse, nexus_tools::ToolError> {
            Ok(ToolResponse {
                output: request.input,
            })
        }
    }

    struct ScriptedProvider {
        responses: Mutex<VecDeque<ModelResponse>>,
        requests: Mutex<Vec<ModelRequest>>,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<ModelResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for ScriptedProvider {
        fn name(&self) -> &str {
            "scripted"
        }

        fn capabilities(&self) -> ModelCapabilities {
            ModelCapabilities::with_tool_calling()
        }

        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
            self.requests.lock().await.push(request);
            self.responses
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| ModelError::Provider("script exhausted".to_owned()))
        }

        async fn stream(&self, request: ModelRequest) -> Result<ModelEventStream, ModelError> {
            let response = self.complete(request).await?;
            Ok(Box::pin(stream::iter(vec![
                Ok(ModelStreamEvent::Started {
                    model: response.model,
                }),
                Ok(ModelStreamEvent::Completed {
                    usage: response.usage,
                }),
            ])))
        }
    }

    fn response(
        content: &str,
        tool_calls: Vec<ModelToolCall>,
        input_tokens: u64,
        output_tokens: u64,
    ) -> ModelResponse {
        ModelResponse {
            model: ModelId("scripted-1".to_owned()),
            message: ChatMessage::new(nexus_models::MessageRole::Assistant, content),
            tool_calls,
            usage: Usage {
                input_tokens,
                output_tokens,
            },
        }
    }

    fn executor(decision: PermissionDecision) -> AuthorizedToolExecutor {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(EchoTool))
            .expect("tool registration should succeed");
        let policy = RuleBasedPolicy::new(decision);
        AuthorizedToolExecutor::new(registry, Arc::new(policy))
    }

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

    #[tokio::test]
    async fn authorized_executor_allows_permitted_tools() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(EchoTool))
            .expect("tool registration should succeed");
        let policy = RuleBasedPolicy::new(PermissionDecision::Deny)
            .with_rule("echo", PermissionDecision::Allow);
        let executor = AuthorizedToolExecutor::new(registry, Arc::new(policy));

        let response = executor
            .execute(
                "echo",
                ToolRequest {
                    input: json!({ "value": 42 }),
                },
            )
            .await
            .expect("permitted tool should execute");

        assert_eq!(response.output["value"], 42);
    }

    #[tokio::test]
    async fn authorized_executor_blocks_denied_tools() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(EchoTool))
            .expect("tool registration should succeed");
        let policy = RuleBasedPolicy::new(PermissionDecision::Deny);
        let executor = AuthorizedToolExecutor::new(registry, Arc::new(policy));

        let error = executor
            .execute(
                "echo",
                ToolRequest {
                    input: json!({ "value": 42 }),
                },
            )
            .await
            .expect_err("denied tool should not execute");

        assert!(matches!(error, RuntimeError::Permission(_)));
    }

    #[tokio::test]
    async fn agent_loop_executes_tools_and_returns_final_answer() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            response(
                "I need the echo tool.",
                vec![ModelToolCall {
                    id: "call-1".to_owned(),
                    name: "echo".to_owned(),
                    arguments: json!({ "value": 42 }),
                }],
                10,
                2,
            ),
            response("The tool returned 42.", Vec::new(), 12, 4),
        ]));
        let runtime = AgentRuntime::new(Config::default(), provider.clone(), "scripted-1");
        let executor = executor(PermissionDecision::Allow);

        let result = runtime
            .run_with_tools("use a tool", &executor, 4)
            .await
            .expect("agent loop should succeed");

        assert_eq!(result.message, "The tool returned 42.");
        assert_eq!(result.usage.input_tokens, 22);
        assert_eq!(result.usage.output_tokens, 6);

        let requests = provider.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].tools[0].name, "echo");
        assert_eq!(requests[1].messages.last().expect("tool result").content, "{\"value\":42}");
    }

    #[tokio::test]
    async fn agent_loop_rejects_tool_calling_when_provider_does_not_support_it() {
        let runtime = AgentRuntime::new(
            Config::default(),
            Arc::new(MockModelProvider::default()),
            "mock-1",
        );
        let executor = executor(PermissionDecision::Allow);

        let error = runtime
            .run_with_tools("use a tool", &executor, 1)
            .await
            .expect_err("mock provider does not support tool calling");

        assert!(matches!(error, RuntimeError::Model(ModelError::Unsupported(_))));
    }

    #[tokio::test]
    async fn agent_loop_stops_at_maximum_steps() {
        let provider = Arc::new(ScriptedProvider::new(vec![response(
            "again",
            vec![ModelToolCall {
                id: "call-1".to_owned(),
                name: "echo".to_owned(),
                arguments: json!({}),
            }],
            1,
            1,
        )]));
        let runtime = AgentRuntime::new(Config::default(), provider, "scripted-1");
        let executor = executor(PermissionDecision::Allow);

        let error = runtime
            .run_with_tools("loop", &executor, 1)
            .await
            .expect_err("loop should hit the step limit");

        assert!(matches!(error, RuntimeError::MaxToolSteps(1)));
    }
}
