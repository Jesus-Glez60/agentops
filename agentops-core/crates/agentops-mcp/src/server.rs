//! JSON-RPC message handling for the stdio MCP transport.

use serde_json::{json, Value};

use crate::protocol::{CallToolResult, JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND};
use crate::tools::{self, AccessMode};

pub fn handle_message(mode: AccessMode, raw: &str) -> Option<String> {
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
                "serverInfo": { "name": "agentops-mcp", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": tools::list_tools(mode) })),
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
        Err(refusal) => JsonRpcResponse::ok(id, serde_json::to_value(CallToolResult::error(refusal)).unwrap()),
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
    fn tools_list_respects_access_mode() {
        let advisor_resp = handle_message(AccessMode::Advisor, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#).unwrap();
        let full_resp = handle_message(AccessMode::Full, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#).unwrap();
        let advisor: Value = serde_json::from_str(&advisor_resp).unwrap();
        let full: Value = serde_json::from_str(&full_resp).unwrap();
        assert!(full["result"]["tools"].as_array().unwrap().len() > advisor["result"]["tools"].as_array().unwrap().len());
    }

    #[test]
    fn calling_an_unknown_tool_returns_an_error_result_not_a_protocol_error() {
        let resp = handle_message(AccessMode::Full, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"does_not_exist","arguments":{}}}"#).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], true);
    }
}
