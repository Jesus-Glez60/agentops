//! `POST /mcp` -- the MCP Streamable HTTP transport (stateless mode: one
//! JSON response per POST, no SSE upgrade -- this server has no
//! server-initiated push, so the optional streaming half of the spec
//! doesn't apply) for a **remote-hosted** agentops instance. This is the
//! network-reachable counterpart to `agentops-mcp-server`'s stdio
//! `Dispatch` -- deliberately not a reuse of that struct, since its
//! `docbrain_store`/`heavy_index` fields are fixed per-process (a single
//! shared DB/Qdrant collection), not tenant-safe. Only `agentops_mcp`'s
//! tools are exposed here -- every one of them takes its own `path`
//! argument, which is what makes them safe to expose to many different
//! tenants from one process (data isolation is per-repo via `GraphStore`'s
//! own `repo`-scoping, unaffected by what's below). Each call still opens
//! its own logical store per invocation from the *caller's* point of view
//! (`agentops_mcp::call_tool` itself is unaware anything is shared) -- but
//! when `AGENTOPS_DATABASE_URL` selects `PostgresGraphStore`, the
//! underlying *connection pool* is now `AppState.pg_store`, shared across
//! every tenant/call in this process via `agentops_mcp::with_shared_postgres_store`
//! (see that function's own doc comment for the real incident this fixes:
//! without it, concurrent `/mcp` tool calls each opened an independent
//! pool and replayed the full schema DDL, causing real Postgres deadlocks
//! under load).
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

use crate::tenant_repo::{register_repo, resolve_connection_path, TenantCaller};
use crate::AppState;

/// Not one of `agentops_mcp::tool_specs()` -- see `handle_tools_call`'s
/// special-casing below for why (it needs `AppState`/tenant access that
/// generic tool signature doesn't carry). Appended to every `tools/list`
/// response here instead, so a client still discovers it via the normal
/// MCP listing rather than needing to know about it out-of-band.
fn register_repo_tool_definition() -> agentops_mcp::ToolDefinition {
    agentops_mcp::ToolDefinition {
        name: "register_repo",
        description: "Registers this repo's git remote URL as a pending connection for your organization, if it isn't connected yet. Use this when another tool call reports a repo/path isn't a recognized connection. Returns the existing connection if one already matches. A registered repo still needs a human to finish connecting it (SSH deploy key or GitHub App) from Repositories -> Connect a repository before it can be scanned/indexed.",
        input_schema: json!({ "type": "object", "properties": { "repo_url": { "type": "string" } }, "required": ["repo_url"] }),
        annotations: agentops_mcp::ToolAnnotations { read_only_hint: false, destructive_hint: false, idempotent_hint: true, open_world_hint: false },
    }
}

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
pub(crate) async fn mcp_handler(Extension(caller): Extension<TenantCaller>, State(state): State<AppState>, body: Result<Json<JsonRpcRequest>, axum::extract::rejection::JsonRejection>) -> Response {
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
        "tools/list" => {
            let mut tools = agentops_mcp::list_tools(resolve_access_mode(&state, &caller.tenant));
            tools.push(register_repo_tool_definition());
            ok(id, json!({ "tools": tools }))
        }
        "tools/call" => handle_tools_call(&state, &caller, id, &request.params).await,
        other => err(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
    };
    Json(response).into_response()
}

/// Resolves the caller's tenant's own `mcp_access_mode` setting
/// (`GET/PUT /team/mcp-access-mode`, admin-toggleable, defaults `"advisor"`
/// -- read-only) instead of the single process-wide `AppState.mode`
/// (`AGENTOPS_ACCESS_MODE` env var). Falls back to `state.mode` when
/// `accounts`/`teams` aren't wired up at all -- the self-hosted,
/// single-operator deployment shape this crate's own `AppState.teams` doc
/// comment already models as `None`, where there's no per-tenant setting to
/// look up and the env var remains the only knob. `add_note`/`ingest_notes`
/// are unaffected by whatever this resolves to either way -- see those two
/// `ToolSpec.access` fields in `agentops-mcp::tools`.
fn resolve_access_mode(state: &AppState, tenant: &str) -> agentops_mcp::AccessMode {
    state
        .teams
        .as_ref()
        .and_then(|teams| teams.lock().unwrap().mcp_access_mode(tenant).ok().flatten())
        .map(|mode| if mode == "full" { agentops_mcp::AccessMode::Full } else { agentops_mcp::AccessMode::Advisor })
        .unwrap_or(state.mode)
}

async fn handle_tools_call(state: &AppState, caller: &TenantCaller, id: Value, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return err(id, INVALID_PARAMS, "missing 'name' in tools/call params");
    };
    let empty = json!({});
    let mut arguments = params.get("arguments").cloned().unwrap_or(empty);

    if name == "register_repo" {
        let Some(repo_url) = arguments.get("repo_url").and_then(|v| v.as_str()) else {
            return err(id, INVALID_PARAMS, "missing 'repo_url' in register_repo arguments");
        };
        let message = register_repo(state, &caller.tenant, repo_url);
        return ok(id, json!({ "content": [{ "type": "text", "text": message }], "isError": false }));
    }

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
    //
    // `with_shared_postgres_store` -- scopes `state.pg_store` as
    // `open_store`'s override for the duration of this one call, so every
    // `tools.rs` handler this dispatches to transparently reuses the shared
    // pool instead of connecting fresh. See that function's own doc
    // comment for the real incident (concurrent `/mcp` calls deadlocking
    // Postgres) this fixes.
    let mode = resolve_access_mode(state, &caller.tenant);
    let name = name.to_string();
    let pg_store = state.pg_store.clone();
    let call_result = tokio::task::spawn_blocking(move || agentops_mcp::with_shared_postgres_store(pg_store.as_ref(), || agentops_mcp::call_tool(mode, &name, &arguments))).await;
    let result = match call_result {
        Ok(Ok(result)) => serde_json::to_value(result).unwrap(),
        Ok(Err(refusal)) => json!({ "content": [{ "type": "text", "text": refusal }], "isError": true }),
        Err(e) => json!({ "content": [{ "type": "text", "text": format!("tool call panicked: {e}") }], "isError": true }),
    };
    ok(id, result)
}
