//! Agent execution runtime with explicit tool boundaries, cancellation, and audit events.

use std::sync::Arc;

use futures_util::StreamExt;
use nexus_config::Config;
use nexus_core::Task;
use nexus_models::{
    ChatMessage, MessageRole, ModelError, ModelId, ModelProvider, ModelRequest, ModelResponse,
    ModelStreamEvent, ModelToolDefinition, Usage,
};
use nexus_permissions::{
    PermissionApprover, PermissionError, PermissionPolicy, PermissionRequest, enforce_with_approver,
};
use nexus_tools::{ToolError, ToolRegistry, ToolRequest, ToolResponse};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub task_id: String,
    pub provider: String,
    pub model: String,
    pub message: String,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub kind: String,
    pub payload: serde_json::Value,
}

pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}

pub struct AgentRuntime {
    config: Config,
    provider: Arc<dyn ModelProvider>,
    model: ModelId,
}

impl AgentRuntime {
    #[must_use]
    pub fn new(config: Config, provider: Arc<dyn ModelProvider>, model: impl Into<ModelId>) -> Self {
        Self {
            config,
            provider,
            model: model.into(),
        }
    }

    pub async fn run(&self, prompt: &str) -> Result<RunResult, RuntimeError> {
        let task = Task::new(prompt);
        let response = self
            .provider
            .complete(ModelRequest::from_prompt(
                self.model.clone(),
                task.prompt.clone(),
            ))
            .await?;
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
        let mut stream = self
            .provider
            .stream(ModelRequest::from_prompt(
                self.model.clone(),
                task.prompt.clone(),
            ))
            .await?;
        let mut message = String::new();
        let mut usage = Usage::default();
        while let Some(event) = stream.next().await {
            let event = event?;
            match &event {
                ModelStreamEvent::Delta { content } => message.push_str(content),
                ModelStreamEvent::Completed { usage: completed } => usage = completed.clone(),
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

    pub async fn run_with_tools(
        &self,
        prompt: &str,
        executor: &AuthorizedToolExecutor,
        max_steps: usize,
    ) -> Result<RunResult, RuntimeError> {
        self.run_with_tools_controlled(
            prompt,
            executor,
            max_steps,
            CancellationToken::new(),
            None,
        )
        .await
    }

    /// Bounded tool loop with cooperative cancellation and structured audit events.
    pub async fn run_with_tools_controlled(
        &self,
        prompt: &str,
        executor: &AuthorizedToolExecutor,
        max_steps: usize,
        cancellation: CancellationToken,
        audit: Option<&dyn AuditSink>,
    ) -> Result<RunResult, RuntimeError> {
        self.run_with_tools_controlled_with_instructions(
            prompt,
            None,
            executor,
            max_steps,
            cancellation,
            audit,
        )
        .await
    }

    /// Controlled execution with optional hierarchical/project/agent instructions.
    pub async fn run_with_tools_controlled_with_instructions(
        &self,
        prompt: &str,
        instructions: Option<&str>,
        executor: &AuthorizedToolExecutor,
        max_steps: usize,
        cancellation: CancellationToken,
        audit: Option<&dyn AuditSink>,
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
        record(
            audit,
            "run.started",
            serde_json::json!({
                "task_id": task.id,
                "max_steps": max_steps,
                "has_instructions": instructions.is_some_and(|value| !value.trim().is_empty())
            }),
        );
        let mut messages = Vec::new();
        if let Some(instructions) = instructions.filter(|value| !value.trim().is_empty()) {
            messages.push(ChatMessage::new(MessageRole::System, instructions));
        }
        messages.push(ChatMessage::new(MessageRole::User, task.prompt.clone()));
        let mut usage = Usage::default();

        for step in 0..max_steps {
            if cancellation.is_cancelled() {
                record(audit, "run.cancelled", serde_json::json!({"step": step}));
                return Err(RuntimeError::Cancelled);
            }
            let request = ModelRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
                temperature: None,
                max_output_tokens: None,
            };
            record(
                audit,
                "model.requested",
                serde_json::json!({"step": step, "messages": request.messages.len()}),
            );
            let response = tokio::select! {
                _ = cancellation.cancelled() => {
                    record(audit, "run.cancelled", serde_json::json!({"step": step, "during": "model"}));
                    return Err(RuntimeError::Cancelled);
                }
                response = self.provider.complete(request) => response?,
            };
            add_usage(&mut usage, &response.usage);
            if response.tool_calls.is_empty() {
                let mut result = self.result_from_response(task.id.to_string(), response);
                result.usage = usage;
                record(
                    audit,
                    "run.completed",
                    serde_json::json!({
                        "task_id": result.task_id,
                        "input_tokens": result.usage.input_tokens,
                        "output_tokens": result.usage.output_tokens
                    }),
                );
                return Ok(result);
            }
            messages.push(response.message);
            for call in response.tool_calls {
                if cancellation.is_cancelled() {
                    record(
                        audit,
                        "run.cancelled",
                        serde_json::json!({"step": step, "during": "tool"}),
                    );
                    return Err(RuntimeError::Cancelled);
                }
                record(
                    audit,
                    "tool.requested",
                    serde_json::json!({"id": call.id, "name": call.name, "arguments": call.arguments}),
                );
                let output = tokio::select! {
                    _ = cancellation.cancelled() => {
                        record(audit, "run.cancelled", serde_json::json!({"step": step, "during": "tool"}));
                        return Err(RuntimeError::Cancelled);
                    }
                    output = executor.execute(&call.name, ToolRequest { input: call.arguments }) => output,
                };
                match output {
                    Ok(output) => {
                        record(
                            audit,
                            "tool.completed",
                            serde_json::json!({"id": call.id, "name": call.name, "output": output.output}),
                        );
                        messages.push(ChatMessage::tool(
                            call.name,
                            call.id,
                            serde_json::to_string(&output.output)?,
                        ));
                    }
                    Err(error) => {
                        record(
                            audit,
                            "tool.failed",
                            serde_json::json!({"id": call.id, "name": call.name, "error": error.to_string()}),
                        );
                        return Err(error);
                    }
                }
            }
        }
        record(
            audit,
            "run.max_steps",
            serde_json::json!({"max_steps": max_steps}),
        );
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

fn record(audit: Option<&dyn AuditSink>, kind: &str, payload: serde_json::Value) {
    if let Some(audit) = audit {
        audit.record(AuditEvent {
            kind: kind.to_owned(),
            payload,
        });
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
    approver: Option<Arc<dyn PermissionApprover>>,
}

impl AuthorizedToolExecutor {
    #[must_use]
    pub fn new(registry: ToolRegistry, policy: Arc<dyn PermissionPolicy>) -> Self {
        Self {
            registry,
            policy,
            approver: None,
        }
    }

    #[must_use]
    pub fn with_approver(mut self, approver: Arc<dyn PermissionApprover>) -> Self {
        self.approver = Some(approver);
        self
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
        enforce_with_approver(
            self.policy.as_ref(),
            self.approver.as_deref(),
            &permission_request,
        )?;
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
    #[error("agent run was cancelled")]
    Cancelled,
}
