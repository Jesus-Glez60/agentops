//! Unified stdio MCP server: merges `agentops-mcp`'s (scan/notes/local
//! search), `docbrain-mcp`'s (library docs), and — when Qdrant is
//! configured — `agentops-heavy-mcp`'s (Qdrant-backed semantic/docs search,
//! model consolidation) tool tables into one process, so a client only
//! needs one stdio server entry instead of up to three.
//!
//! **Access-control scope, deliberately minimal**: `agentops-mcp`'s own
//! `AccessMode` (Advisor/Full) gating stays exactly where it already is —
//! only its tools respect it. `docbrain-mcp`'s and `agentops-heavy-mcp`'s
//! tools have no such concept today (neither crate's `ToolSpec`/
//! `ToolDefinition` even has an access field) and are available regardless
//! of mode; retrofitting a shared capability system into two more crates is
//! out of scope for this merge, whose goal is removing tiering, not adding
//! a new cross-cutting one.
//!
//! **Tool-name collisions, resolved before this crate existed** (see each
//! source tool's own doc comment for the rationale): `agentops-mcp`'s
//! former `semantic_search` is now `local_semantic_search` (heavy's Qdrant-
//! backed one keeps the short name); `agentops-mcp`'s former `get_changelog`
//! is now `list_scans` (docbrain's own `get_changelog`, an unrelated
//! library-versions concept, keeps its name); `agentops-heavy-mcp`'s former
//! `search_docs`/`index_docs` are now `search_docs_indexed`/
//! `index_docs_indexed` (docbrain's native sqlite-vec-backed `search_docs`
//! keeps its name — these two may turn out to be redundant, a follow-up
//! product decision, not resolved here).

mod protocol;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use agentops_mcp::AccessMode;
use docbrain_graph::SqliteDocbrainStore;
use serde_json::{json, Value};

use protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND};

pub struct Dispatch {
    pub mode: AccessMode,
    pub docbrain_store: SqliteDocbrainStore,
    pub docbrain_db_path: PathBuf,
    /// `None` when `AGENTOPS_QDRANT_URL` isn't set — heavy's tools are
    /// simply absent from `tools/list`/`tools/call` rather than the process
    /// refusing to start, same posture `agentops-heavy-api`'s REST `/search*`
    /// routes already take for the identical env var.
    pub heavy_index: Option<agentops_heavy_embeddings::SemanticIndex>,
}

impl Dispatch {
    fn list_tools(&self) -> Vec<Value> {
        let mut tools: Vec<Value> = agentops_mcp::list_tools(self.mode).into_iter().map(|t| serde_json::to_value(t).unwrap()).collect();
        tools.extend(docbrain_mcp::list_tools().into_iter().map(|t| serde_json::to_value(t).unwrap()));
        if self.heavy_index.is_some() {
            tools.extend(agentops_heavy_mcp::list_tools().into_iter().map(|t| serde_json::to_value(t).unwrap()));
        }
        tools
    }

    /// Dispatches by checking each tool table's own name list first
    /// (`agentops-mcp` -> `docbrain-mcp` -> `agentops-heavy-mcp`), not by
    /// sniffing "unknown tool" error strings — an unknown name is rejected
    /// once, not silently retried against every backend.
    async fn call_tool(&mut self, name: &str, args: &Value) -> Value {
        if agentops_mcp::list_tools(self.mode).iter().any(|t| t.name == name) {
            return match agentops_mcp::call_tool(self.mode, name, args) {
                Ok(result) => serde_json::to_value(result).unwrap(),
                Err(refusal) => json!({ "content": [{ "type": "text", "text": refusal }], "isError": true }),
            };
        }
        if docbrain_mcp::list_tools().iter().any(|t| t.name == name) {
            return match docbrain_mcp::call_tool(&self.docbrain_store, &self.docbrain_db_path, name, args) {
                Ok(result) => serde_json::to_value(result).unwrap(),
                Err(refusal) => json!({ "content": [{ "type": "text", "text": refusal }], "isError": true }),
            };
        }
        if let Some(index) = &mut self.heavy_index {
            if agentops_heavy_mcp::list_tools().iter().any(|t| t.name == name) {
                let result = agentops_heavy_mcp::call_tool(index, name, args).await;
                return serde_json::to_value(result).unwrap();
            }
        }
        json!({ "content": [{ "type": "text", "text": format!("unknown tool '{name}'") }], "isError": true })
    }
}

async fn handle_message(dispatch: &mut Dispatch, raw: &str) -> Option<String> {
    let request: JsonRpcRequest = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(e) => {
            let resp = JsonRpcResponse::err(Value::Null, INTERNAL_ERROR, format!("parse error: {e}"));
            return Some(serde_json::to_string(&resp).unwrap());
        }
    };
    let id = request.id.clone()?;

    let response = match request.method.as_str() {
        "initialize" => JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "agentops-mcp-server", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": dispatch.list_tools() })),
        "tools/call" => handle_tools_call(dispatch, id, &request.params).await,
        other => JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
    };
    Some(serde_json::to_string(&response).unwrap())
}

async fn handle_tools_call(dispatch: &mut Dispatch, id: Value, params: &Value) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return JsonRpcResponse::err(id, INVALID_PARAMS, "missing 'name' in tools/call params");
    };
    let empty = json!({});
    let arguments = params.get("arguments").unwrap_or(&empty);
    JsonRpcResponse::ok(id, dispatch.call_tool(name, arguments).await)
}

/// Runs the merged server over stdin/stdout until stdin closes. One
/// request per line (newline-delimited JSON), matching the framing every
/// MCP stdio client already speaks.
pub async fn run_stdio(mut dispatch: Dispatch) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_message(&mut dispatch, &line).await {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use docbrain_graph::DocbrainStore;

    fn test_dispatch() -> Dispatch {
        Dispatch { mode: AccessMode::Full, docbrain_store: SqliteDocbrainStore::open_in_memory().unwrap(), docbrain_db_path: PathBuf::from("unused.db"), heavy_index: None }
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let mut dispatch = test_dispatch();
        let resp = handle_message(&mut dispatch, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "agentops-mcp-server");
    }

    #[tokio::test]
    async fn tools_list_combines_agentops_and_docbrain_tools_and_excludes_heavy_when_qdrant_is_unconfigured() {
        let mut dispatch = test_dispatch();
        let resp = handle_message(&mut dispatch, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let names: Vec<&str> = v["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"scan_repo"), "agentops-mcp tools must be present: {names:?}");
        assert!(names.contains(&"list_libraries"), "docbrain-mcp tools must be present: {names:?}");
        assert!(names.contains(&"local_semantic_search"), "the renamed agentops-mcp tool must use its new name: {names:?}");
        assert!(!names.contains(&"semantic_search"), "heavy-mcp's semantic_search must be absent with no Qdrant configured: {names:?}");
        assert!(names.iter().collect::<std::collections::HashSet<_>>().len() == names.len(), "no duplicate tool names across merged tables: {names:?}");
    }

    #[tokio::test]
    async fn calling_an_unknown_tool_returns_an_error_result_not_a_protocol_error() {
        let mut dispatch = test_dispatch();
        let resp = handle_message(&mut dispatch, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"does_not_exist","arguments":{}}}"#).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], true);
    }

    #[tokio::test]
    async fn calling_a_docbrain_tool_reuses_docbrain_mcps_dispatch() {
        let mut dispatch = test_dispatch();
        dispatch.docbrain_store.add_library("react", "React", None, None, Some("https://react.dev")).unwrap();
        let resp = handle_message(&mut dispatch, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_library","arguments":{"slug":"react"}}}"#).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false, "{v:?}");
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("react"));
    }
}
