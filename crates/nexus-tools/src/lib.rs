//! Tool contracts, registry primitives, and built-in local tools.

use std::{collections::HashMap, path::{Path, PathBuf}, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    pub output: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMetadata {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEvent {
    pub tool: String,
    pub phase: ToolEventPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEventPhase {
    Started,
    Completed,
    Failed,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn metadata(&self) -> ToolMetadata;

    async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolError>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let name = tool.metadata().name;
        if self.tools.contains_key(&name) {
            return Err(ToolError::AlreadyRegistered(name));
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn Tool>, ToolError> {
        self.tools
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::NotFound(name.to_owned()))
    }

    pub async fn execute(&self, name: &str, request: ToolRequest) -> Result<ToolResponse, ToolError> {
        self.get(name)?.execute(request).await
    }

    #[must_use]
    pub fn metadata(&self) -> Vec<ToolMetadata> {
        let mut metadata = self.tools.values().map(|tool| tool.metadata()).collect::<Vec<_>>();
        metadata.sort_by(|left, right| left.name.cmp(&right.name));
        metadata
    }
}

/// Read-only filesystem tool rooted inside a Nexus workspace.
pub struct FileSystemTool {
    root: PathBuf,
}

impl FileSystemTool {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, requested: &str) -> Result<PathBuf, ToolError> {
        let path = Path::new(requested);
        if path.is_absolute() {
            return Err(ToolError::InvalidInput("absolute paths are not allowed".to_owned()));
        }

        let root = self.root.canonicalize().map_err(|error| ToolError::Execution(error.to_string()))?;
        let candidate = root.join(path);
        let resolved = candidate
            .canonicalize()
            .map_err(|error| ToolError::Execution(error.to_string()))?;

        if !resolved.starts_with(&root) {
            return Err(ToolError::InvalidInput("path escapes workspace root".to_owned()));
        }

        Ok(resolved)
    }
}

#[async_trait]
impl Tool for FileSystemTool {
    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            name: "filesystem.read".to_owned(),
            description: "Read a UTF-8 file inside the configured workspace".to_owned(),
        }
    }

    async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
        let path = request
            .input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing string field `path`".to_owned()))?;
        let resolved = self.resolve(path)?;
        let content = tokio::fs::read_to_string(resolved)
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;

        Ok(ToolResponse {
            output: json!({ "content": content }),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("tool already registered: {0}")]
    AlreadyRegistered(String),
    #[error("invalid tool input: {0}")]
    InvalidInput(String),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::{Tool, ToolError, ToolMetadata, ToolRegistry, ToolRequest, ToolResponse};

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn metadata(&self) -> ToolMetadata {
            ToolMetadata {
                name: "echo".to_owned(),
                description: "echoes input".to_owned(),
            }
        }

        async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
            Ok(ToolResponse { output: request.input })
        }
    }

    #[tokio::test]
    async fn registry_executes_registered_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool)).expect("registration should succeed");

        let response = registry
            .execute("echo", ToolRequest { input: json!({ "value": 42 }) })
            .await
            .expect("execution should succeed");

        assert_eq!(response.output["value"], 42);
    }

    #[test]
    fn duplicate_tool_names_are_rejected() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool)).expect("first registration should succeed");
        assert!(matches!(registry.register(Arc::new(EchoTool)), Err(ToolError::AlreadyRegistered(_))));
    }
}
