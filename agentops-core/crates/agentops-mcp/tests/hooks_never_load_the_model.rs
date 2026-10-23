//! A local hook process must never load the embedding model (ADR 0052):
//! `hooks::handle` is what `agentops hook-capture` runs locally, and only
//! the server calls `handle_with` with an embedder. This lives in its own
//! test binary, so no other test can have loaded the model first.

use agentops_graph::{upsert_node, EdgeRelation, GraphStore, NewNode, NodeKind, SqliteGraphStore};
use serde_json::json;

#[test]
fn every_store_hook_runs_without_loading_the_embedding_model() {
    use agentops_mcp::hooks::{handle, REL_PATH_FIELD, STORE_EVENTS};
    let store = SqliteGraphStore::open_in_memory().unwrap();
    let repo = "r";
    let note = |name: &str, content: &str| upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: repo.into(), path: Some(format!("vault:{name}.md")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap();
    let a = note("Hooks ignore the remote server", "hook capture opens the local store in src/hook_capture.rs");
    let b = note("Remote hooks time out", "remote hook server timeout");
    store.add_edge(repo, a, b, EdgeRelation::SimilarTo).unwrap();
    let mut v = vec![0.0f32; agentops_embeddings::EMBEDDING_DIM];
    v[0] = 1.0;
    store.set_embedding(repo, a, &v).unwrap();
    store.set_embedding(repo, b, &v).unwrap();

    // Every event that touches the store, with the payloads that reach its
    // recall paths. A new entry in `STORE_EVENTS` fails here until it has one.
    let cases = [
        ("userpromptsubmit", vec![
            // One session per prompt: a default-mode prompt leaves out what
            // its session has already been shown.
            json!({ "session_id": "s", "prompt": "make the hooks follow the remote server", "permission_mode": "plan" }),
            json!({ "session_id": "t", "prompt": "why do remote hooks time out", "permission_mode": "default" }),
        ]),
        ("subagentstart", vec![json!({ "session_id": "s", "agent_type": "Explore", "agent_id": "a1" })]),
        // A session of its own, so the prompt seeds haven't shown the file's
        // memories already.
        ("posttooluse", vec![
            json!({ "session_id": "w", "tool_name": "Read", "tool_input": {"file_path": "/r/src/hook_capture.rs"}, "tool_response": {"type": "text", "text": "fn main() {}"}, REL_PATH_FIELD: "src/hook_capture.rs" }),
            json!({ "session_id": "w", "tool_name": "Read", "tool_input": {"file_path": "/r/src/hook_capture.rs"}, "tool_response": "x", "agent_id": "a1", REL_PATH_FIELD: "src/hook_capture.rs" }),
            json!({ "session_id": "w", "tool_name": "Edit", "tool_input": {"file_path": "/r/src/hook_capture.rs"}, "tool_response": "ok", REL_PATH_FIELD: "src/hook_capture.rs" }),
            json!({ "session_id": "w", "tool_name": "Bash", "tool_input": {"command": "cargo test"}, "tool_response": "remote hook server timeout\n".repeat(400) }),
            json!({ "session_id": "w", "tool_name": "mcp__agentops__add_note", "tool_input": {"path": repo}, "tool_response": [{ "type": "text", "text": format!("Wrote x.md.\nresult: {{\"node_id\":{b},\"written\":true}}") }] }),
        ]),
        ("posttoolusefailure", vec![json!({ "session_id": "w", "tool_name": "Bash", "error": "Exit code 1\nremote hook server timeout after 5s" })]),
        ("sessionstart", vec![json!({ "session_id": "s", "source": "compact" }), json!({ "session_id": "u", "source": "startup" })]),
        ("pretooluse", vec![json!({ "session_id": "s", "tool_name": "ExitPlanMode", "tool_input": {"plan": "# Plan\nmake the hooks follow the remote server"} })]),
    ];
    for event in STORE_EVENTS {
        let (_, payloads) = cases.iter().find(|(e, _)| e == event).unwrap_or_else(|| panic!("no payload drives the {event} hook"));
        for payload in payloads {
            handle(&store, repo, event, payload).unwrap_or_else(|e| panic!("{event}: {e}"));
        }
    }
    let work = store.session_events(repo, "w").unwrap();
    assert!(work.iter().any(|e| e.event_kind == agentops_mcp::hooks::RECALL_INJECTED && e.tool_name == agentops_mcp::work_recall::WORK_RECALL_TOOL), "a work cue reached recall and showed something: {work:?}");

    assert!(!agentops_embeddings::model_loaded(), "a local hook loaded the embedding model");
}
