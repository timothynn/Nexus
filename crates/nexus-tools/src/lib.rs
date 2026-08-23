//! Tool contracts and registry primitives.

use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ToolRequest {
    pub input: Value,
}

#[derive(Debug, Clone)]
pub struct ToolResponse {
    pub output: Value,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool execution failed: {0}")]
    Execution(String),
}
