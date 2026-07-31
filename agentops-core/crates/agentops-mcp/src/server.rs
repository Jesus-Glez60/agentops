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

    /// Regression test for a real bug: `scan_repo` used to duplicate its
    /// own `add_node` loop instead of sharing `agentops_mcp::scan::persist`
    /// with `agentops-cli`'s `install`, so it never got the upsert/prune
    /// fix that `install` did — meaning the actual primary way this
    /// product gets used (an agent calling `scan_repo` via MCP mid-session)
    /// still silently duplicated every file/symbol node on every rescan.
    /// Drives the real JSON-RPC dispatch path (not the library directly),
    /// since that's the boundary where the two implementations diverged.
    #[test]
    fn rescanning_via_the_scan_repo_tool_does_not_duplicate_nodes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let scan_req = |id: u64| format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"scan_repo","arguments":{{"path":"{path}"}}}}}}"#);

        handle_message(AccessMode::Full, &scan_req(1)).unwrap();
        handle_message(AccessMode::Full, &scan_req(2)).unwrap();

        let status_req = format!(r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"status","arguments":{{"path":"{path}"}}}}}}"#);
        let resp = handle_message(AccessMode::Full, &status_req).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("symbols: 1"), "rescanning twice must not double the symbol count: {text}");
    }

    #[test]
    fn get_dependencies_reports_a_real_resolved_dependency_edge() {
        // resolve_dependency_edges only resolves `./`/`../`-style relative
        // imports (see ranker.rs) — TypeScript exercises the well-covered
        // path (the same style the ranker's own tests use), unlike
        // Python's `from .utils import x`, which this join-based resolver
        // doesn't handle correctly.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("utils.ts"), "export function helper() {}\n").unwrap();
        std::fs::write(dir.path().join("main.ts"), "import { helper } from './utils';\n\nexport function run() { helper(); }\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let scan_req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"scan_repo","arguments":{{"path":"{path}"}}}}}}"#);
        handle_message(AccessMode::Full, &scan_req).unwrap();

        let main_deps_req = format!(r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"get_dependencies","arguments":{{"path":"{path}","file":"main.ts"}}}}}}"#);
        let resp = handle_message(AccessMode::Full, &main_deps_req).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("utils.ts"), "main.ts should depend on utils.ts: {text}");

        let utils_deps_req = format!(r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"get_dependencies","arguments":{{"path":"{path}","file":"utils.ts"}}}}}}"#);
        let resp = handle_message(AccessMode::Full, &utils_deps_req).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("main.ts"), "utils.ts should be depended on by main.ts: {text}");
    }

    #[test]
    fn get_dependencies_on_an_unknown_file_is_a_tool_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "x = 1\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let scan_req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"scan_repo","arguments":{{"path":"{path}"}}}}}}"#);
        handle_message(AccessMode::Full, &scan_req).unwrap();

        let deps_req = format!(r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"get_dependencies","arguments":{{"path":"{path}","file":"nope.py"}}}}}}"#);
        let resp = handle_message(AccessMode::Full, &deps_req).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], true);
    }

    #[test]
    fn get_symbol_returns_the_exact_symbol_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token(t):\n    return t == 'ok'\n\ndef verify_password(p):\n    return len(p) > 8\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let scan_req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"scan_repo","arguments":{{"path":"{path}"}}}}}}"#);
        handle_message(AccessMode::Full, &scan_req).unwrap();

        let req = format!(r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"get_symbol","arguments":{{"path":"{path}","name":"verify_token"}}}}}}"#);
        let resp = handle_message(AccessMode::Full, &req).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false, "{v:?}");
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("verify_token"), "{text}");
        assert!(text.contains("auth.py"), "{text}");
        assert!(text.contains("return t == 'ok'"), "get_symbol should return the full source, not just the location: {text}");
        assert!(!text.contains("verify_password"), "get_symbol must not return unrelated symbols: {text}");
    }

    #[test]
    fn get_symbol_on_an_unknown_name_is_a_tool_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "x = 1\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let scan_req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"scan_repo","arguments":{{"path":"{path}"}}}}}}"#);
        handle_message(AccessMode::Full, &scan_req).unwrap();

        let req = format!(r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"get_symbol","arguments":{{"path":"{path}","name":"does_not_exist"}}}}}}"#);
        let resp = handle_message(AccessMode::Full, &req).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], true);
    }

    #[test]
    fn ast_search_finds_symbols_by_case_insensitive_substring() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token(t):\n    return True\n\ndef verify_password(p):\n    return True\n\ndef unrelated():\n    return True\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let scan_req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"scan_repo","arguments":{{"path":"{path}"}}}}}}"#);
        handle_message(AccessMode::Full, &scan_req).unwrap();

        let req = format!(r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"ast_search","arguments":{{"path":"{path}","query":"VERIFY"}}}}}}"#);
        let resp = handle_message(AccessMode::Full, &req).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false, "{v:?}");
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("verify_token"), "{text}");
        assert!(text.contains("verify_password"), "{text}");
        assert!(!text.contains("unrelated"), "ast_search must not return non-matching symbols: {text}");
    }

    #[test]
    fn ast_search_with_no_matches_returns_a_result_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def foo():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let scan_req = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"scan_repo","arguments":{{"path":"{path}"}}}}}}"#);
        handle_message(AccessMode::Full, &scan_req).unwrap();

        let req = format!(r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"ast_search","arguments":{{"path":"{path}","query":"zzz_nothing_matches"}}}}}}"#);
        let resp = handle_message(AccessMode::Full, &req).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false, "no matches is a valid empty result, not a tool error: {v:?}");
    }
}
