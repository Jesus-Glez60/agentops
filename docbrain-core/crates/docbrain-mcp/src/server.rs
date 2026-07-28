use docbrain_graph::DocbrainStore;
use serde_json::{json, Value};

use crate::protocol::{
    CallToolResult, JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, MCP_PROTOCOL_VERSION,
    METHOD_NOT_FOUND,
};
use crate::tools;

pub fn handle_message(store: &DocbrainStore, raw: &str) -> Option<String> {
    let request: JsonRpcRequest = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(e) => {
            let resp = JsonRpcResponse::err(Value::Null, INTERNAL_ERROR, format!("parse error: {e}"));
            return Some(serde_json::to_string(&resp).unwrap());
        }
    };

    let Some(id) = request.id.clone() else {
        return None;
    };

    let response = match request.method.as_str() {
        "initialize" => JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "docbrain-mcp", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": tools::list_tools() })),
        "tools/call" => handle_tools_call(store, id, &request.params),
        other => JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
    };

    Some(serde_json::to_string(&response).unwrap())
}

fn handle_tools_call(store: &DocbrainStore, id: Value, params: &Value) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return JsonRpcResponse::err(id, INVALID_PARAMS, "missing 'name' in tools/call params");
    };
    let empty = json!({});
    let arguments = params.get("arguments").unwrap_or(&empty);

    match tools::call_tool(store, name, arguments) {
        Ok(result) => JsonRpcResponse::ok(id, serde_json::to_value(result).unwrap()),
        Err(refusal) => JsonRpcResponse::ok(id, serde_json::to_value(CallToolResult::error(refusal)).unwrap()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_server_info() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let resp = handle_message(&store, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "docbrain-mcp");
    }

    #[test]
    fn tools_list_has_five_tools() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let resp = handle_message(&store, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 5);
    }
}
