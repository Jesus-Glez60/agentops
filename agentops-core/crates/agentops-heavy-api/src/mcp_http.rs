//! `POST /mcp` -- the MCP Streamable HTTP transport (stateless mode: one
//! JSON response per POST, no SSE upgrade -- this server has no
//! server-initiated push, so the optional streaming half of the spec
//! doesn't apply) for a **remote-hosted** agentops instance. This is the
//! network-reachable counterpart to `agentops-mcp-server`'s stdio
//! `Dispatch` -- deliberately not a reuse of that struct, since its
//! `docbrain_store`/`heavy_index` fields are fixed per-process (a single
//! shared DB/Qdrant collection), not tenant-safe. Only `agentops_mcp`'s
//! tools are exposed here -- every one of them takes its own `path`
//! argument and opens its own store per call, which is what makes them
//! safe to expose to many different tenants from one process (see
//! `agentops-api`'s own doc comment for the same design, reused unchanged).
//!
//! **The `path` tool argument is never trusted as a literal filesystem
//! path here.** A local/stdio MCP client is a single trusted user's own
//! process -- a remote HTTP client is not. `path` must instead match a
//! `RepoConnection` id (or its `repo_url`) belonging to the caller's
//! tenant; it's resolved via `indexing::checkout_path` before reaching
//! `agentops_mcp::call_tool`. This is what stops a remote caller from
//! probing arbitrary paths on the server's filesystem.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::indexing::checkout_path;
use crate::{AppState, McpCaller};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

#[derive(Deserialize)]
pub(crate) struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

/// A JSON-RPC *notification* (no `id`, e.g. `notifications/initialized`)
/// gets `202 Accepted` with no body per the Streamable HTTP spec, not a
/// JSON-RPC response -- there's nothing to reply to.
pub(crate) async fn mcp_handler(Extension(caller): Extension<McpCaller>, State(state): State<AppState>, body: Result<Json<JsonRpcRequest>, axum::extract::rejection::JsonRejection>) -> Response {
    let Json(request) = match body {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(err(Value::Null, INTERNAL_ERROR, format!("parse error: {e}")))).into_response(),
    };
    let Some(id) = request.id else {
        return StatusCode::ACCEPTED.into_response();
    };

    let response = match request.method.as_str() {
        "initialize" => ok(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "agentops-heavy-api", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        "tools/list" => ok(id, json!({ "tools": agentops_mcp::list_tools(state.mode) })),
        "tools/call" => handle_tools_call(&state, &caller, id, &request.params).await,
        other => err(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
    };
    Json(response).into_response()
}

async fn handle_tools_call(state: &AppState, caller: &McpCaller, id: Value, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return err(id, INVALID_PARAMS, "missing 'name' in tools/call params");
    };
    let empty = json!({});
    let mut arguments = params.get("arguments").cloned().unwrap_or(empty);

    if let Some(connection_ref) = arguments.get("path").and_then(|v| v.as_str()).map(str::to_string) {
        match resolve_connection_path(state, &caller.tenant, &connection_ref) {
            Ok(resolved) => {
                arguments["path"] = json!(resolved.display().to_string());
            }
            Err(message) => {
                return ok(id, json!({ "content": [{ "type": "text", "text": message }], "isError": true }));
            }
        }
    }

    // `spawn_blocking` -- required, not just a performance nicety. See
    // this module's doc comment note on `agentops_mcp::call_tool`; a real
    // panic ("Cannot start a runtime from within a runtime") caught live
    // against a Postgres-backed deployment is what surfaced this, here and
    // in `agentops-api`'s pre-existing `/tools/{name}` route.
    let mode = state.mode;
    let name = name.to_string();
    let call_result = tokio::task::spawn_blocking(move || agentops_mcp::call_tool(mode, &name, &arguments)).await;
    let result = match call_result {
        Ok(Ok(result)) => serde_json::to_value(result).unwrap(),
        Ok(Err(refusal)) => json!({ "content": [{ "type": "text", "text": refusal }], "isError": true }),
        Err(e) => json!({ "content": [{ "type": "text", "text": format!("tool call panicked: {e}") }], "isError": true }),
    };
    ok(id, result)
}

/// `path` must name a `RepoConnection` (by id or its `repo_url`) belonging
/// to `tenant` -- anything else is rejected, never treated as a literal
/// filesystem path. See this module's doc comment for why.
fn resolve_connection_path(state: &AppState, tenant: &str, connection_ref: &str) -> Result<std::path::PathBuf, String> {
    let store = state.store.lock().unwrap();
    let connection = store
        .get_connection(tenant, connection_ref)
        .ok()
        .flatten()
        .or_else(|| store.list_connections(tenant).ok()?.into_iter().find(|c| c.repo_url == connection_ref));
    let Some(connection) = connection else {
        return Err(format!("'{connection_ref}' is not a repo connection id or URL for your organization -- use one of the ids/URLs from GET /repos"));
    };
    Ok(checkout_path(&state.repo_checkouts_dir, tenant, &connection.id))
}
