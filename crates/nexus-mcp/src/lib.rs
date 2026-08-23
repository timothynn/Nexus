//! MCP client primitives and Nexus tool adapters.

use std::{collections::BTreeMap, process::Stdio, sync::Arc};

use async_trait::async_trait;
use nexus_tools::{Tool, ToolError, ToolMetadata, ToolRequest, ToolResponse};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

#[derive(Debug, Clone)]
pub struct McpServerCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl McpServerCommand {
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self { program: program.into(), args: Vec::new(), env: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)] pub description: String,
    #[serde(rename = "inputSchema", default = "default_schema")]
    pub input_schema: Value,
}

fn default_schema() -> Value { json!({ "type": "object" }) }

pub struct McpToolAdapter {
    command: McpServerCommand,
    metadata: ToolMetadata,
    remote_name: String,
}

impl McpToolAdapter {
    #[must_use]
    pub fn new(command: McpServerCommand, remote: McpTool) -> Self {
        Self {
            metadata: ToolMetadata { name: format!("mcp.{}", remote.name), description: remote.description, input_schema: remote.input_schema },
            remote_name: remote.name,
            command,
        }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn metadata(&self) -> ToolMetadata { self.metadata.clone() }

    async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
        let mut client = StdioMcpClient::connect(&self.command).await.map_err(|error| ToolError::Execution(error.to_string()))?;
        client.initialize("nexus").await.map_err(|error| ToolError::Execution(error.to_string()))?;
        let output = client.call_tool(&self.remote_name, request.input).await.map_err(|error| ToolError::Execution(error.to_string()))?;
        let _ = client.shutdown().await;
        Ok(ToolResponse { output })
    }
}

pub async fn discover_tool_adapters(command: McpServerCommand) -> Result<Vec<Arc<dyn Tool>>, McpError> {
    let mut client = StdioMcpClient::connect(&command).await?;
    client.initialize("nexus").await?;
    let tools = client.list_tools().await?;
    let _ = client.shutdown().await;
    Ok(tools.into_iter().map(|tool| Arc::new(McpToolAdapter::new(command.clone(), tool)) as Arc<dyn Tool>).collect())
}

pub struct StdioMcpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl StdioMcpClient {
    pub async fn connect(command: &McpServerCommand) -> Result<Self, McpError> {
        let mut process = Command::new(&command.program);
        process.args(&command.args).envs(&command.env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut child = process.spawn().map_err(McpError::Io)?;
        let stdin = child.stdin.take().ok_or_else(|| McpError::Protocol("server stdin unavailable".to_owned()))?;
        let stdout = child.stdout.take().ok_or_else(|| McpError::Protocol("server stdout unavailable".to_owned()))?;
        Ok(Self { child, stdin, stdout: BufReader::new(stdout).lines(), next_id: 1 })
    }

    pub async fn initialize(&mut self, client_name: &str) -> Result<Value, McpError> {
        self.request("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":client_name,"version":env!("CARGO_PKG_VERSION")}})).await
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpTool>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        serde_json::from_value(result.get("tools").cloned().unwrap_or_else(|| json!([]))).map_err(|error| McpError::Protocol(error.to_string()))
    }

    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.request("tools/call", json!({"name":name,"arguments":arguments})).await
    }

    pub async fn shutdown(mut self) -> Result<(), McpError> {
        self.stdin.shutdown().await.map_err(McpError::Io)?;
        self.child.kill().await.map_err(McpError::Io)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let encoded = serde_json::to_string(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).map_err(|error| McpError::Protocol(error.to_string()))?;
        self.stdin.write_all(encoded.as_bytes()).await.map_err(McpError::Io)?;
        self.stdin.write_all(b"\n").await.map_err(McpError::Io)?;
        self.stdin.flush().await.map_err(McpError::Io)?;
        loop {
            let Some(line) = self.stdout.next_line().await.map_err(McpError::Io)? else { return Err(McpError::Protocol("server closed stdout".to_owned())); };
            let response: JsonRpcResponse = serde_json::from_str(&line).map_err(|error| McpError::Protocol(error.to_string()))?;
            if response.id != Some(id) { continue; }
            if let Some(error) = response.error { return Err(McpError::Remote(error.message)); }
            return response.result.ok_or_else(|| McpError::Protocol("response missing result".to_owned()));
        }
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse { id: Option<u64>, result: Option<Value>, error: Option<JsonRpcError> }
#[derive(Debug, Deserialize)]
struct JsonRpcError { message: String }

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP I/O failed: {0}")] Io(std::io::Error),
    #[error("MCP protocol error: {0}")] Protocol(String),
    #[error("MCP server returned an error: {0}")] Remote(String),
}

#[cfg(test)]
mod tests {
    use super::{McpServerCommand, McpTool, McpToolAdapter};
    use nexus_tools::Tool;
    use serde_json::json;

    #[test]
    fn adapter_namespaces_remote_tools() {
        let tool = McpToolAdapter::new(McpServerCommand::new("server"), McpTool { name: "search".to_owned(), description: "Search".to_owned(), input_schema: json!({}) });
        assert_eq!(tool.metadata().name, "mcp.search");
    }
}
