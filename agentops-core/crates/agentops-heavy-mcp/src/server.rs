//! Request dispatch — `handle_message` is the pure(ish), testable core; the
//! stdio read/write loop in `lib.rs` is a thin wrapper around it.

use agentops_heavy_embeddings::SemanticIndex;
use serde_json::{json, Value};

use crate::protocol::{JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND, INVALID_PARAMS, INTERNAL_ERROR};
use crate::tools;

/// Handles one raw JSON-RPC message. Returns `None` for notifications (no
/// `id`), which the MCP spec says never get a response.
pub async fn handle_message(index: &mut SemanticIndex, raw: &str) -> Option<String> {
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
                "serverInfo": { "name": "agentops-heavy-mcp", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": tools::list_tools() })),
        "tools/call" => handle_tools_call(index, id, &request.params).await,
        other => JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
    };

    Some(serde_json::to_string(&response).unwrap())
}

async fn handle_tools_call(index: &mut SemanticIndex, id: Value, params: &Value) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return JsonRpcResponse::err(id, INVALID_PARAMS, "missing 'name' in tools/call params");
    };
    let empty = json!({});
    let arguments = params.get("arguments").unwrap_or(&empty);

    let result = tools::call_tool(index, name, arguments).await;
    JsonRpcResponse::ok(id, serde_json::to_value(result).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests hit a REAL Qdrant instance and the REAL downloaded BGE-M3
    // model. Set AGENTOPS_TEST_QDRANT_URL to run; skipped otherwise.
    fn test_index() -> Option<SemanticIndex> {
        let url = std::env::var("AGENTOPS_TEST_QDRANT_URL").ok()?;
        Some(SemanticIndex::connect(&url, "agentops_heavy_mcp_test").expect("connect"))
    }

    #[tokio::test]
    async fn initialize_returns_server_info() {
        let Some(mut index) = test_index() else { return };
        let resp = handle_message(&mut index, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "agentops-heavy-mcp");
    }

    #[tokio::test]
    async fn notifications_get_no_response() {
        let Some(mut index) = test_index() else { return };
        let resp = handle_message(&mut index, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn tools_list_includes_all_five_tools() {
        let Some(mut index) = test_index() else { return };
        let resp = handle_message(&mut index, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let names: Vec<&str> = v["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"semantic_search"));
        assert!(names.contains(&"semantic_index"));
        assert!(names.contains(&"search_docs_indexed"));
        assert!(names.contains(&"index_docs_indexed"));
        assert!(names.contains(&"consolidate_model"));
    }

    #[tokio::test]
    async fn index_then_search_over_the_real_mcp_dispatch_path() {
        let Some(mut index) = test_index() else { return };
        index.ensure_collection().await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().to_string_lossy().to_string();
        // Nodes are stored under the canonicalized repo name, not the raw
        // path — the real bug caught via live testing against this repo
        // itself (tool_semantic_index now uses repo_name too).
        let repo = agentops_heavy_embeddings::repo_name(dir.path());
        {
            use agentops_graph::GraphStore;
            let db_path = dir.path().join(".context").join("graph.db");
            let store = agentops_graph::SqliteGraphStore::open(&db_path).unwrap();
            store
                .add_node(agentops_graph::NewNode {
                    kind: agentops_graph::NodeKind::Gotcha,
                    repo: repo.clone(),
                    path: None,
                    name: Some("rate-limit-backoff".into()),
                    container: None,
                    start_line: None,
                    end_line: None,
                    content: Some("Retry with exponential backoff when the upstream API returns 429.".into()),
                })
                .unwrap();
        }

        let index_req = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"semantic_index","arguments":{"path": repo_path}}});
        let resp = handle_message(&mut index, &index_req.to_string()).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false, "{v:?}");

        let search_req = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"semantic_search","arguments":{"path": repo_path, "query": "what should I do if the API rate limits me", "session_id": "sess-semantic-hit"}}});
        let resp = handle_message(&mut index, &search_req.to_string()).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false, "{v:?}");
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("rate-limit-backoff"), "{text}");

        // Module 8: a real hit was found, and session_id was passed -- must
        // have recorded a "hit"-kind session_events row against the same
        // repo, with the top result's node id attached.
        {
            use agentops_graph::GraphStore;
            let db_path = dir.path().join(".context").join("graph.db");
            let store = agentops_graph::SqliteGraphStore::open(&db_path).unwrap();
            let events = store.session_events(&repo, "sess-semantic-hit").unwrap();
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!(events[0].tool_name, "semantic_search");
            assert_eq!(events[0].event_kind, "hit");
            assert!(events[0].node_id.is_some(), "{events:?}");
        }
    }

    #[tokio::test]
    async fn index_docs_then_search_docs_over_the_real_mcp_dispatch_path() {
        let Some(mut index) = test_index() else { return };
        index.ensure_collection().await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("docbrain.db");
        {
            use docbrain_graph::{DocbrainStore, NewDocNode, NodeKind as DocNodeKind};
            let store = docbrain_graph::SqliteDocbrainStore::open(&db_path).unwrap();
            store.add_library("next", "Next.js", None, None, Some("https://nextjs.org/docs")).unwrap();
            store
                .upsert_doc_node(
                    "next",
                    NewDocNode {
                        version: "15.1.3",
                        topic: "App Router caching",
                        content: "Use the fetch cache option to control revalidation behavior.",
                        content_hash: "hash1",
                        token_count: 10,
                        embedding: None,
                        kind: DocNodeKind::Prose,
                    },
                )
                .unwrap();
        }

        let index_req = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"index_docs_indexed","arguments":{"slug": "next", "db_path": db_path.to_str().unwrap()}}});
        let resp = handle_message(&mut index, &index_req.to_string()).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false, "{v:?}");

        let search_req = json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"search_docs_indexed","arguments":{"slug": "next", "query": "how do I control fetch revalidation"}}});
        let resp = handle_message(&mut index, &search_req.to_string()).await.unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["isError"], false, "{v:?}");
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("App Router caching"), "{text}");
    }

    #[tokio::test]
    async fn search_docs_never_returns_a_codebrain_symbol_result() {
        let Some(mut index) = test_index() else { return };
        index.ensure_collection().await.unwrap();

        // Index a codebrain symbol with text that would otherwise be a
        // strong semantic match for the docbrain query below.
        let dir = tempfile::tempdir().unwrap();
        {
            use agentops_graph::GraphStore;
            let db_path = dir.path().join(".context").join("graph.db");
            let store = agentops_graph::SqliteGraphStore::open(&db_path).unwrap();
            store
                .add_node(agentops_graph::NewNode {
                    kind: agentops_graph::NodeKind::Symbol,
                    repo: "kind-isolation-probe".into(),
                    path: Some("src/cache.rs".into()),
                    name: Some("fetch cache revalidation".into()),
                    container: None,
                    start_line: None,
                    end_line: None,
                    content: Some("fetch cache revalidation control function".into()),
                })
                .unwrap();
        }
        let items = agentops_heavy_embeddings::collect_index_items(
            &agentops_graph::SqliteGraphStore::open(&dir.path().join(".context").join("graph.db")).unwrap(),
            "kind-isolation-probe",
        )
        .unwrap();
        index.index_items(&items).await.unwrap();

        let hits = index.search_scoped("fetch cache revalidation control", 5, None, Some("doc")).await.unwrap();
        assert!(hits.iter().all(|h| h.kind == "doc"), "a symbol leaked into a doc-only query: {hits:?}");
    }
}
