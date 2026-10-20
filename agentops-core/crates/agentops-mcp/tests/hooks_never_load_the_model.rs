//! A local hook process must never load the embedding model (ADR 0052):
//! `hooks::handle` is what `agentops hook-capture` runs locally, and only
//! the server calls `handle_with` with an embedder. This lives in its own
//! test binary, so no other test can have loaded the model first.

use agentops_graph::{upsert_node, EdgeRelation, GraphStore, NewNode, NodeKind, SqliteGraphStore};
use serde_json::json;

#[test]
fn every_seeding_hook_runs_without_loading_the_embedding_model() {
    let store = SqliteGraphStore::open_in_memory().unwrap();
    let repo = "r";
    let note = |name: &str, content: &str| upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: repo.into(), path: Some(format!("vault:{name}.md")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap();
    let a = note("Hooks ignore the remote server", "hook capture opens the local store");
    let b = note("Remote hooks time out", "remote hook server timeout");
    store.add_edge(repo, a, b, EdgeRelation::SimilarTo).unwrap();
    let mut v = vec![0.0f32; agentops_embeddings::EMBEDDING_DIM];
    v[0] = 1.0;
    store.set_embedding(repo, a, &v).unwrap();
    store.set_embedding(repo, b, &v).unwrap();

    // One session per prompt: a default-mode prompt leaves out what its
    // session has already been shown.
    for (session, prompt, mode) in [("s", "make the hooks follow the remote server", "plan"), ("t", "why do remote hooks time out", "default")] {
        let out = agentops_mcp::hooks::handle(&store, repo, "userpromptsubmit", &json!({ "session_id": session, "prompt": prompt, "permission_mode": mode })).unwrap();
        assert!(out.is_some(), "{mode} prompt seeded nothing");
    }
    agentops_mcp::hooks::handle(&store, repo, "subagentstart", &json!({ "session_id": "s", "agent_type": "Explore", "agent_id": "a1" })).unwrap();

    assert!(!agentops_embeddings::model_loaded(), "a local hook loaded the embedding model");
}
