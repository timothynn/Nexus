//! Tool contracts, registry primitives, and built-in local tools.

use std::{
    collections::{hash_map::Entry, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{process::Command, time::timeout};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    pub output: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolMetadata {
    pub name: String,
    pub description: String,
    #[serde(default = "default_input_schema")]
    pub input_schema: Value,
}

fn default_input_schema() -> Value {
    json!({ "type": "object" })
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
        match self.tools.entry(name) {
            Entry::Occupied(entry) => Err(ToolError::AlreadyRegistered(entry.key().to_owned())),
            Entry::Vacant(entry) => {
                entry.insert(tool);
                Ok(())
            }
        }
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn Tool>, ToolError> {
        self.tools
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::NotFound(name.to_owned()))
    }

    pub async fn execute(
        &self,
        name: &str,
        request: ToolRequest,
    ) -> Result<ToolResponse, ToolError> {
        self.get(name)?.execute(request).await
    }

    #[must_use]
    pub fn metadata(&self) -> Vec<ToolMetadata> {
        let mut metadata = self
            .tools
            .values()
            .map(|tool| tool.metadata())
            .collect::<Vec<_>>();
        metadata.sort_by(|left, right| left.name.cmp(&right.name));
        metadata
    }
}

fn resolve_workspace_path(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let path = Path::new(requested);
    if path.is_absolute() {
        return Err(ToolError::InvalidInput(
            "absolute paths are not allowed".to_owned(),
        ));
    }

    let root = root
        .canonicalize()
        .map_err(|error| ToolError::Execution(error.to_string()))?;
    let candidate = root.join(path);
    let resolved = candidate
        .canonicalize()
        .map_err(|error| ToolError::Execution(error.to_string()))?;

    if !resolved.starts_with(&root) {
        return Err(ToolError::InvalidInput(
            "path escapes workspace root".to_owned(),
        ));
    }

    Ok(resolved)
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
}

#[async_trait]
impl Tool for FileSystemTool {
    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            name: "filesystem.read".to_owned(),
            description: "Read a UTF-8 file inside the configured workspace".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
        let path = request
            .input
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing string field `path`".to_owned()))?;
        let resolved = resolve_workspace_path(&self.root, path)?;
        let content = tokio::fs::read_to_string(resolved)
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;

        Ok(ToolResponse {
            output: json!({ "content": content }),
        })
    }
}

/// Structured process execution rooted inside a Nexus workspace.
///
/// The tool never invokes a command shell. Programs and arguments are passed
/// directly to the operating system, which avoids shell interpolation and keeps
/// approval policies attached to the actual executable request.
pub struct ShellTool {
    root: PathBuf,
    default_timeout: Duration,
    max_timeout: Duration,
}

impl ShellTool {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            default_timeout: Duration::from_secs(30),
            max_timeout: Duration::from_secs(300),
        }
    }

    #[must_use]
    pub fn with_timeouts(
        root: impl Into<PathBuf>,
        default_timeout: Duration,
        max_timeout: Duration,
    ) -> Self {
        Self {
            root: root.into(),
            default_timeout,
            max_timeout,
        }
    }

    fn timeout_for(&self, request: &Value) -> Result<Duration, ToolError> {
        let Some(timeout_ms) = request.get("timeout_ms") else {
            return Ok(self.default_timeout);
        };
        let timeout_ms = timeout_ms.as_u64().ok_or_else(|| {
            ToolError::InvalidInput("`timeout_ms` must be an unsigned integer".to_owned())
        })?;
        if timeout_ms == 0 {
            return Err(ToolError::InvalidInput(
                "`timeout_ms` must be greater than zero".to_owned(),
            ));
        }

        let duration = Duration::from_millis(timeout_ms);
        if duration > self.max_timeout {
            return Err(ToolError::InvalidInput(format!(
                "`timeout_ms` exceeds the maximum of {} ms",
                self.max_timeout.as_millis()
            )));
        }

        Ok(duration)
    }

    fn working_directory(&self, request: &Value) -> Result<PathBuf, ToolError> {
        let Some(cwd) = request.get("cwd") else {
            return self
                .root
                .canonicalize()
                .map_err(|error| ToolError::Execution(error.to_string()));
        };
        let cwd = cwd
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("`cwd` must be a string".to_owned()))?;
        resolve_workspace_path(&self.root, cwd)
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            name: "shell.execute".to_owned(),
            description: "Run a structured program with arguments inside the configured workspace"
                .to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "program": { "type": "string", "minLength": 1 },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "default": []
                    },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1 }
                },
                "required": ["program"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
        let program = request
            .input
            .get("program")
            .and_then(Value::as_str)
            .filter(|program| !program.is_empty())
            .ok_or_else(|| ToolError::InvalidInput("missing string field `program`".to_owned()))?;
        let args = request
            .input
            .get("args")
            .map(|args| {
                args
                    .as_array()
                    .ok_or_else(|| ToolError::InvalidInput("`args` must be an array".to_owned()))?
                    .iter()
                    .map(|argument| {
                        argument.as_str().map(str::to_owned).ok_or_else(|| {
                            ToolError::InvalidInput("every `args` entry must be a string".to_owned())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let cwd = self.working_directory(&request.input)?;
        let timeout_duration = self.timeout_for(&request.input)?;

        let mut command = Command::new(program);
        command.args(&args).current_dir(cwd);
        let output = timeout(timeout_duration, command.output())
            .await
            .map_err(|_| ToolError::Timeout(timeout_duration))?
            .map_err(|error| ToolError::Execution(error.to_string()))?;

        Ok(ToolResponse {
            output: json!({
                "success": output.status.success(),
                "exit_code": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr)
            }),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error("tool execution timed out after {0:?}")]
    Timeout(Duration),
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("tool already registered: {0}")]
    AlreadyRegistered(String),
    #[error("invalid tool input: {0}")]
    InvalidInput(String),
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use async_trait::async_trait;
    use serde_json::json;

    use super::{
        ShellTool, Tool, ToolError, ToolMetadata, ToolRegistry, ToolRequest, ToolResponse,
    };

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn metadata(&self) -> ToolMetadata {
            ToolMetadata {
                name: "echo".to_owned(),
                description: "echoes input".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "value": {} }
                }),
            }
        }

        async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
            Ok(ToolResponse {
                output: request.input,
            })
        }
    }

    #[tokio::test]
    async fn registry_executes_registered_tool() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(EchoTool))
            .expect("registration should succeed");

        let response = registry
            .execute(
                "echo",
                ToolRequest {
                    input: json!({ "value": 42 }),
                },
            )
            .await
            .expect("execution should succeed");

        assert_eq!(response.output["value"], 42);
    }

    #[test]
    fn duplicate_tool_names_are_rejected() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(EchoTool))
            .expect("first registration should succeed");
        assert!(matches!(
            registry.register(Arc::new(EchoTool)),
            Err(ToolError::AlreadyRegistered(_))
        ));
    }

    #[tokio::test]
    async fn shell_tool_runs_structured_command_without_a_shell() {
        let root = std::env::current_dir().expect("current directory should exist");
        let tool = ShellTool::new(root);
        let response = tool
            .execute(ToolRequest {
                input: json!({
                    "program": "printf",
                    "args": ["nexus"]
                }),
            })
            .await
            .expect("structured command should execute");

        assert_eq!(response.output["stdout"], "nexus");
        assert_eq!(response.output["exit_code"], 0);
    }

    #[tokio::test]
    async fn shell_tool_rejects_timeouts_above_policy_limit() {
        let root = std::env::current_dir().expect("current directory should exist");
        let tool = ShellTool::with_timeouts(
            root,
            Duration::from_millis(10),
            Duration::from_millis(20),
        );
        let error = tool
            .execute(ToolRequest {
                input: json!({
                    "program": "printf",
                    "timeout_ms": 21
                }),
            })
            .await
            .expect_err("timeout above the policy limit should fail");

        assert!(matches!(error, ToolError::InvalidInput(_)));
    }
}
