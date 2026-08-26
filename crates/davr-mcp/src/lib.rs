use davr_core::CoreEngine;
use davr_git::RollbackScope;
use davr_types::{DavrError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

pub struct McpServer {
    project_root: PathBuf,
    engine: CoreEngine,
}

impl McpServer {
    pub fn new(project_root: impl AsRef<Path>) -> Self {
        let root = project_root.as_ref().to_path_buf();
        let engine = CoreEngine::new(&root);
        Self {
            project_root: root,
            engine,
        }
    }

    /// Starts the stdio JSON-RPC MCP server loop
    pub async fn run_stdio(&self) -> Result<()> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin).lines();

        info!("DAVR MCP server listening on stdio");

        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let request: JsonRpcRequest = match serde_json::from_str(line) {
                Ok(req) => req,
                Err(e) => {
                    let err_resp = JsonRpcResponse {
                        jsonrpc: "2.0".into(),
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {}", e),
                            data: None,
                        }),
                    };
                    let resp_str = serde_json::to_string(&err_resp).unwrap_or_default() + "\n";
                    let _ = stdout.write_all(resp_str.as_bytes()).await;
                    let _ = stdout.flush().await;
                    continue;
                }
            };

            let response = self.handle_request(request).await;
            let resp_str = serde_json::to_string(&response).unwrap_or_default() + "\n";
            let _ = stdout.write_all(resp_str.as_bytes()).await;
            let _ = stdout.flush().await;
        }

        Ok(())
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone();
        match req.method.as_str() {
            "initialize" => JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(json!({
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {
                        "name": "davr-mcp",
                        "version": "0.1.0"
                    },
                    "capabilities": {
                        "tools": {}
                    }
                })),
                error: None,
            },

            "tools/list" => JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(json!({
                    "tools": [
                        {
                            "name": "davr_doctor",
                            "description": "Run pre-flight environment checks across compiler, runtime, git, and credentials",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "davr_test",
                            "description": "Execute test suites across detected frameworks (cargo test, pytest, jest, go test)",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "framework": { "type": "string", "description": "cargo_test, pytest, jest, go_test" },
                                    "filter": { "type": "string", "description": "Test name filter pattern" }
                                }
                            }
                        },
                        {
                            "name": "davr_analyze_impact",
                            "description": "Compute transitive change impact analysis and select affected tests",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "snapshot": { "type": "string", "description": "Base snapshot tree hash" },
                                    "depth": { "type": "integer", "description": "Max transitive graph depth (default: 3)" }
                                }
                            }
                        },
                        {
                            "name": "davr_rollback",
                            "description": "Safely revert agent-modified files while preserving independent developer edits (A ∩ B)",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "snapshot": { "type": "string", "description": "Target snapshot tree hash" },
                                    "dry_run": { "type": "boolean", "description": "Preview without writing to disk" }
                                }
                            }
                        },
                        {
                            "name": "davr_session_status",
                            "description": "List recent supervised AI agent execution sessions and telemetry",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "limit": { "type": "integer", "description": "Max sessions to return" }
                                }
                            }
                        }
                    ]
                })),
                error: None,
            },

            "tools/call" => {
                let params = req.params.unwrap_or(json!({}));
                let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));

                match self.dispatch_tool(tool_name, args).await {
                    Ok(tool_result) => JsonRpcResponse {
                        jsonrpc: "2.0".into(),
                        id,
                        result: Some(json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": serde_json::to_string_pretty(&tool_result).unwrap_or_default()
                                }
                            ]
                        })),
                        error: None,
                    },
                    Err(e) => JsonRpcResponse {
                        jsonrpc: "2.0".into(),
                        id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32603,
                            message: format!("Tool execution error: {}", e),
                            data: None,
                        }),
                    },
                }
            }

            _ => JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", req.method),
                    data: None,
                }),
            },
        }
    }

    async fn dispatch_tool(&self, name: &str, args: Value) -> Result<Value> {
        match name {
            "davr_doctor" => {
                let results = self.engine.doctor(None)?;
                Ok(serde_json::to_value(results).unwrap_or_default())
            }

            "davr_test" => {
                let framework = args.get("framework").and_then(|v| v.as_str());
                let filter = args.get("filter").and_then(|v| v.as_str());
                let results = self.engine.run_tests(framework, filter).await?;
                Ok(serde_json::to_value(results).unwrap_or_default())
            }

            "davr_analyze_impact" => {
                let snapshot = args.get("snapshot").and_then(|v| v.as_str());
                let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
                let report = self.engine.analyze_impact(snapshot, depth)?;
                Ok(serde_json::to_value(report).unwrap_or_default())
            }

            "davr_rollback" => {
                let snapshot = args.get("snapshot").and_then(|v| v.as_str());
                let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
                let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                let report = self.engine.rollback(
                    snapshot,
                    None,
                    RollbackScope::SessionIntersection,
                    dry_run,
                    force,
                )?;
                Ok(serde_json::to_value(report).unwrap_or_default())
            }

            "davr_session_status" => {
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                let sessions = self.engine.list_sessions(limit)?;
                Ok(serde_json::to_value(sessions).unwrap_or_default())
            }

            _ => Err(DavrError::General(format!("Unknown tool: {}", name))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_mcp_initialize_and_tools_list() {
        let temp = TempDir::new().unwrap();
        let server = McpServer::new(temp.path());

        let init_req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "initialize".into(),
            params: None,
        };
        let init_resp = server.handle_request(init_req).await;
        assert!(init_resp.result.is_some());

        let list_req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(2)),
            method: "tools/list".into(),
            params: None,
        };
        let list_resp = server.handle_request(list_req).await;
        assert!(list_resp.result.is_some());
    }
}
