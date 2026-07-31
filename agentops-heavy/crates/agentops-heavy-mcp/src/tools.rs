//! `semantic_search`/`semantic_index` tool definitions and dispatch. Unlike
//! `agentops-mcp`'s `AccessMode` gating (a structural, always-on capability
//! boundary), this server's whole tool set only exists if a valid license
//! was found at startup — there's no "advisor mode" distinction here, just
//! licensed vs. not, since semantic search has no write-capable variant to
//! restrict in the first place.

use agentops_embeddings::SemanticIndex;
use agentops_graph::SqliteGraphStore;
use docbrain_graph::{DocbrainStore, TenantContext};
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
        ToolDefinition {
            name: "search_docs",
            description: "Find the most relevant library documentation sections for a plain-language query, ranked by meaning rather than keyword overlap. Requires index_docs to have been run for this library at least once. Never returns codebrain symbol/gotcha results, even from the same underlying index.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": { "type": "string", "description": "The library's docbrain slug." },
                    "query": { "type": "string" },
                    "top_k": { "type": "integer", "description": "Defaults to 5." },
                    "org": { "type": "string" },
                },
                "required": ["slug", "query"],
            }),
        },
        ToolDefinition {
            name: "index_docs",
            description: "Embed and index every doc node docbrain has for a library (across all its scraped versions), so search_docs has something to find. Run this after scrape_library.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "db_path": { "type": "string", "description": "Path to the docbrain SQLite store. Defaults to ~/.agentops/docbrain.db." },
                    "org": { "type": "string" },
                },
                "required": ["slug"],
            }),
        },
    ]
}

pub async fn call_tool(index: &mut SemanticIndex, name: &str, args: &Value) -> CallToolResult {
    let result = match name {
        "semantic_search" => tool_semantic_search(index, args).await,
        "semantic_index" => tool_semantic_index(index, args).await,
        "search_docs" => tool_search_docs(index, args).await,
        "index_docs" => tool_index_docs(index, args).await,
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

fn default_docbrain_db_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".agentops").join("docbrain.db")
}

async fn tool_search_docs(index: &mut SemanticIndex, args: &Value) -> anyhow::Result<String> {
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug' argument"))?;
    let query = get_str(args, "query").ok_or_else(|| anyhow::anyhow!("missing required 'query' argument"))?;
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5);

    let hits = index.search_scoped(query, top_k, Some(slug), Some("doc")).await?;
    if hits.is_empty() {
        return Ok("No results — has index_docs been run for this library?".to_string());
    }

    let mut out = String::new();
    for hit in hits {
        let topic = hit.name.as_deref().unwrap_or("(untitled)");
        let version = hit.path.as_deref().unwrap_or("(unknown version)");
        out.push_str(&format!("{:.3}  {slug}@{version} — {topic}\n", hit.score));
        out.push_str(&format!("      {}\n\n", hit.text.lines().next().unwrap_or("")));
    }
    Ok(out)
}

async fn tool_index_docs(index: &mut SemanticIndex, args: &Value) -> anyhow::Result<String> {
    let slug = get_str(args, "slug").ok_or_else(|| anyhow::anyhow!("missing required 'slug' argument"))?;
    let db_path = get_str(args, "db_path").map(std::path::PathBuf::from).unwrap_or_else(default_docbrain_db_path);
    let tenant = match get_str(args, "org") {
        Some(org) => TenantContext::org(org),
        None => TenantContext::public(),
    };

    // Same discipline as tool_semantic_index: collecting items from
    // DocbrainStore is fully synchronous and finishes — including dropping
    // the store — before the async embedding step below. DocbrainStore
    // wraps a rusqlite::Connection (!Sync), so a reference to it must never
    // cross an .await.
    let items = {
        let store = DocbrainStore::open(&db_path).map_err(|e| anyhow::anyhow!("opening docbrain store at {}: {e}", db_path.display()))?;
        agentops_embeddings::collect_doc_index_items(&store, &tenant, slug)?
    };

    if items.is_empty() {
        return Ok(format!("No doc nodes found for '{slug}' — has scrape_library or ingest_local_files been run for it?"));
    }

    let count = index.index_items(&items).await?;
    Ok(format!("Indexed {count} doc section(s) for {slug}"))
}
