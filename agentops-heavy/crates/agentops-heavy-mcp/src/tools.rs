//! `semantic_search`/`semantic_index` tool definitions and dispatch. Unlike
//! `agentops-mcp`'s `AccessMode` gating (a structural, always-on capability
//! boundary), this server's whole tool set only exists if a valid license
//! was found at startup — there's no "advisor mode" distinction here, just
//! licensed vs. not, since semantic search has no write-capable variant to
//! restrict in the first place.

use agentops_embeddings::SemanticIndex;
use agentops_graph::SqliteGraphStore;
use serde_json::{json, Value};

use crate::protocol::{CallToolResult, ToolDefinition};

pub fn list_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "semantic_search",
            description: "Find the most relevant symbols/gotchas/decisions for a plain-language query, ranked by meaning rather than keyword overlap. Requires semantic_index to have been run for this repo at least once.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "query": { "type": "string" },
                    "top_k": { "type": "integer", "description": "Defaults to 5." },
                },
                "required": ["path", "query"],
            }),
        },
        ToolDefinition {
            name: "semantic_index",
            description: "Embed and index every symbol/gotcha/decision node from an already-scanned repo's graph store, so semantic_search has something to find. Run this after agentops install, and again after any rescan.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        },
    ]
}

pub async fn call_tool(index: &mut SemanticIndex, name: &str, args: &Value) -> CallToolResult {
    let result = match name {
        "semantic_search" => tool_semantic_search(index, args).await,
        "semantic_index" => tool_semantic_index(index, args).await,
        other => return CallToolResult::error(format!("unknown tool '{other}'")),
    };

    match result {
        Ok(text) => CallToolResult::success(text),
        Err(e) => CallToolResult::error(e.to_string()),
    }
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

async fn tool_semantic_search(index: &mut SemanticIndex, args: &Value) -> anyhow::Result<String> {
    let path = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path' argument"))?;
    let query = get_str(args, "query").ok_or_else(|| anyhow::anyhow!("missing required 'query' argument"))?;
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5);

    let hits = index.search(query, top_k, Some(path)).await?;
    if hits.is_empty() {
        return Ok("No results — has semantic_index been run for this repo?".to_string());
    }

    let mut out = String::new();
    for hit in hits {
        let label = hit.name.as_deref().unwrap_or("(unnamed)");
        out.push_str(&format!("{:.3}  [{}] {}\n", hit.score, hit.kind, label));
        if let Some(p) = &hit.path {
            out.push_str(&format!("      {p}\n"));
        }
        out.push_str(&format!("      {}\n\n", hit.text.lines().next().unwrap_or("")));
    }
    Ok(out)
}

async fn tool_semantic_index(index: &mut SemanticIndex, args: &Value) -> anyhow::Result<String> {
    let path = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path' argument"))?;

    // Collecting items is synchronous and fully finishes — including
    // dropping the graph store — before the async embedding step below.
    // &dyn GraphStore isn't provably Sync (SQLite connections aren't), so
    // it must never be held across an .await — see
    // agentops_embeddings::collect_index_items's doc comment for why this
    // isn't just style preference.
    let items = {
        let db_path = std::path::Path::new(path).join(".context").join("graph.db");
        let store = SqliteGraphStore::open(&db_path).map_err(|e| anyhow::anyhow!("opening graph store at {}: {e}", db_path.display()))?;
        agentops_embeddings::collect_index_items(&store, path)?
    };

    let count = index.index_items(&items).await?;
    Ok(format!("Indexed {count} nodes from {path}"))
}
