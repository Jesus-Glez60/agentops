//! Minimal JSON-RPC 2.0 / MCP wire types — hand-rolled rather than a
//! pre-1.0 MCP SDK crate, matching `docbrain-mcp`'s identical approach
//! (deliberately duplicated, not shared — the two products stay
//! independently versionable).
//!
//! `ToolDefinition` carries real MCP `ToolAnnotations`
//! (`readOnlyHint`/`destructiveHint`/`idempotentHint`/`openWorldHint`, per
//! the current MCP spec) — a gap `docbrain-mcp`'s `ToolDefinition` still
//! has, worth fixing there opportunistically, not blocking here.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    pub fn err(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self { jsonrpc: "2.0", id, result: None, error: Some(JsonRpcError { code, message: message.into() }) }
    }
}

pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug, Serialize, Clone, Copy)]
pub struct ToolAnnotations {
    #[serde(rename = "readOnlyHint")]
    pub read_only_hint: bool,
    #[serde(rename = "destructiveHint")]
    pub destructive_hint: bool,
    #[serde(rename = "idempotentHint")]
    pub idempotent_hint: bool,
    #[serde(rename = "openWorldHint")]
    pub open_world_hint: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    pub annotations: ToolAnnotations,
}

#[derive(Debug, Serialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self { kind: "text", text: text.into() }
    }
}

#[derive(Debug, Serialize)]
pub struct CallToolResult {
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

impl CallToolResult {
    pub fn success(text: impl Into<String>) -> Self {
        Self { content: vec![ContentBlock::text(text)], is_error: false }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self { content: vec![ContentBlock::text(text)], is_error: true }
    }
}
