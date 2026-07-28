//! Request dispatch — `handle_message` is the pure, testable core; the stdio
//! read/write loop in `lib.rs` is a thin wrapper around it.

use serde_json::{json, Value};

use crate::protocol::{
    JsonRpcRequest, JsonRpcResponse, CallToolResult, MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND, INTERNAL_ERROR,
    INVALID_PARAMS,
};
use crate::tools::{self, AccessMode};

/// Handles one raw JSON-RPC message. Returns `None` for notifications (no
/// `id`, e.g. `notifications/initialized`), which the MCP spec says never get
/// a response.
pub fn handle_message(mode: AccessMode, raw: &str) -> Option<String> {
    let request: JsonRpcRequest = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(e) => {
            // Malformed JSON with no recoverable id — respond with a null-id error
            // per JSON-RPC convention rather than silently dropping it.
            let resp = JsonRpcResponse::err(Value::Null, INTERNAL_ERROR, format!("parse error: {e}"));
            return Some(serde_json::to_string(&resp).unwrap());
        }
    };

    let Some(id) = request.id.clone() else {
        // Notification — handle side effects if any (none needed for
        // notifications/initialized), no response.
        return None;
    };

    let response = match request.method.as_str() {
        "initialize" => JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "agentops-mcp", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        "tools/list" => {
            let defs = tools::list_tools(mode);
            JsonRpcResponse::ok(id, json!({ "tools": defs }))
        }
        "tools/call" => handle_tools_call(mode, id, &request.params),
        other => JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
    };

    Some(serde_json::to_string(&response).unwrap())
}

fn handle_tools_call(mode: AccessMode, id: Value, params: &Value) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return JsonRpcResponse::err(id, INVALID_PARAMS, "missing 'name' in tools/call params");
    };
    let empty = json!({});
    let arguments = params.get("arguments").unwrap_or(&empty);

    match tools::call_tool(mode, name, arguments) {
        Ok(result) => JsonRpcResponse::ok(id, serde_json::to_value(result).unwrap()),
        Err(refusal) => {
            // A refused tool (not registered for this AccessMode, or unknown)
            // still gets a well-formed tool result, not a transport-level
            // error — this is a normal, expected outcome an agent should be
            // able to read and act on, not a protocol failure.
            let result = CallToolResult::error(refusal);
            JsonRpcResponse::ok(id, serde_json::to_value(result).unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_server_info() {
        let resp = handle_message(AccessMode::Full, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "agentops-mcp");
    }

    #[test]
    fn notifications_get_no_response() {
        let resp = handle_message(AccessMode::Full, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(resp.is_none());
    }

    #[test]
    fn tools_list_reflects_access_mode() {
        let resp = handle_message(AccessMode::Advisor, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let names: Vec<&str> = v["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"status"));
        assert!(!names.contains(&"scan_repo"));
    }

    #[test]
    fn calling_write_tool_in_advisor_mode_returns_a_tool_error_not_a_protocol_error() {
        let raw = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"scan_repo","arguments":{"path":"/tmp"}}}"#;
        let resp = handle_message(AccessMode::Advisor, raw).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert!(v.get("error").is_none(), "should be a valid JSON-RPC response, not a transport error");
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("Advisor"));
    }

    #[test]
    fn unknown_method_is_a_protocol_error() {
        let resp = handle_message(AccessMode::Full, r#"{"jsonrpc":"2.0","id":4,"method":"nope","params":{}}"#).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert!(v.get("error").is_some());
    }
}
