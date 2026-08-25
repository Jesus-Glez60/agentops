//! `semantic_search`/`semantic_index` tool definitions and dispatch. Unlike
//! `agentops-mcp`'s `AccessMode` gating (a structural, always-on capability
//! boundary), this server's whole tool set is available whenever Qdrant is
//! configured — no advisor-mode distinction, since semantic search has no
//! write-capable variant to restrict in the first place.

use agentops_graph::SqliteGraphStore;
use agentops_heavy_embeddings::SemanticIndex;
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
            // Renamed from `search_docs` — collides with `docbrain-mcp`'s
            // own `search_docs` tool (native sqlite-vec search over
            // docbrain's own graph store, no Qdrant required) once both
            // tool tables are merged into one dispatcher. This one and
            // docbrain's may be genuinely redundant (two doc-search
            // backends), not just a naming accident — flagged as a
            // follow-up product decision, not resolved here.
            name: "search_docs_indexed",
            description: "Find the most relevant library documentation sections for a plain-language query, ranked by meaning rather than keyword overlap. Requires index_docs_indexed to have been run for this library at least once. Never returns codebrain symbol/gotcha results, even from the same underlying index.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": { "type": "string", "description": "The library's docbrain slug." },
                    "query": { "type": "string" },
                    "top_k": { "type": "integer", "description": "Defaults to 5." },
                },
                "required": ["slug", "query"],
            }),
        },
        ToolDefinition {
            name: "index_docs_indexed",
            description: "Embed and index every doc node docbrain has for a library (across all its scraped versions), so search_docs_indexed has something to find. Run this after scrape_library.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "db_path": { "type": "string", "description": "Path to the docbrain SQLite store. Defaults to ~/.agentops/docbrain/default.db." },
                    "org": { "type": "string", "description": "Selects a per-tenant docbrain file (~/.agentops/docbrain/<org>.db) when db_path isn't given explicitly." },
                },
                "required": ["slug"],
            }),
        },
        ToolDefinition {
            name: "consolidate_model",
            description: "On-demand: LoRA fine-tune a small local code model on this repo's own curated Gotcha/Decision notes (CLS-inspired plan, Initiative 6), so future code-explanation assistance can be grounded in this repo's own reviewed knowledge instead of only a frontier model's frozen training data. Downloads the base model on first run, trains a fresh adapter, and only promotes it if it doesn't regress a held-out eval versus whatever's currently active. Heavier and slower than semantic_index/end_session (real CPU training, not just embedding) — never run automatically, invoke explicitly when there's enough newly curated knowledge to be worth consolidating.",
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
        "search_docs_indexed" => tool_search_docs(index, args).await,
        "index_docs_indexed" => tool_index_docs(index, args).await,
        "consolidate_model" => tool_consolidate_model(args).await,
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

    // Must be the canonicalized repo name, matching what tool_semantic_index
    // stored nodes under — see agentops_heavy_embeddings::repo_name's doc
    // comment; a real bug caught via live testing against this repo.
    let repo = agentops_heavy_embeddings::repo_name(std::path::Path::new(path));
    let hits = index.search(query, top_k, Some(&repo)).await?;
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
    // agentops_heavy_embeddings::collect_index_items's doc comment for why
    // this isn't just style preference.
    let items = {
        let repo_path = std::path::Path::new(path);
        let db_path = repo_path.join(".context").join("graph.db");
        let store = SqliteGraphStore::open(&db_path).map_err(|e| anyhow::anyhow!("opening graph store at {}: {e}", db_path.display()))?;
        let repo = agentops_heavy_embeddings::repo_name(repo_path);
        agentops_heavy_embeddings::collect_index_items(&store, &repo)?
    };

    let count = index.index_items(&items).await?;
    Ok(format!("Indexed {count} nodes from {path}"))
}

/// One docbrain SQLite file per tenant under `~/.agentops/docbrain/` —
/// `<org>.db`, or `default.db` when no `org` is given — same convention
/// `agentops-heavy-api` uses, and for the same reason: `docbrain-graph` is
/// single-tenant this rebuild (no `TenantContext`), so multi-tenant
/// isolation is achieved by routing to a different file, not a trait
/// parameter. An explicit `db_path` argument always wins over this.
fn default_docbrain_db_path(org: Option<&str>) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::PathBuf::from(home).join(".agentops").join("docbrain");
    match org {
        Some(org) => dir.join(format!("{org}.db")),
        None => dir.join("default.db"),
    }
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
    let org = get_str(args, "org");
    let db_path = get_str(args, "db_path").map(std::path::PathBuf::from).unwrap_or_else(|| default_docbrain_db_path(org));

    // Same discipline as tool_semantic_index: collecting items from the
    // docbrain store is fully synchronous and finishes — including
    // dropping the store — before the async embedding step below. It
    // wraps a rusqlite::Connection (!Sync), so a reference to it must
    // never cross an .await.
    let items = {
        let store = docbrain_graph::SqliteDocbrainStore::open(&db_path).map_err(|e| anyhow::anyhow!("opening docbrain store at {}: {e}", db_path.display()))?;
        agentops_heavy_embeddings::collect_doc_index_items(&store, slug)?
    };

    if items.is_empty() {
        return Ok(format!("No doc nodes found for '{slug}' — has scrape_library or ingest_local_files been run for it?"));
    }

    let count = index.index_items(&items).await?;
    Ok(format!("Indexed {count} doc section(s) for {slug}"))
}

/// Runs a full Initiative 6 consolidation pass. Unlike every other tool in
/// this file, this one is fully synchronous internally (`consolidate_model`
/// does its own blocking model download/training/eval, no async I/O at
/// all) — no `&mut SemanticIndex` needed, and no `.await` inside the
/// `{ ... }` block that owns the `SqliteGraphStore` for the same
/// !Sync-across-await reason `tool_semantic_index` documents, even though
/// here the whole call happens to be synchronous anyway.
async fn tool_consolidate_model(args: &Value) -> anyhow::Result<String> {
    let path = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path' argument"))?;

    let repo_path = std::path::Path::new(path);
    let db_path = repo_path.join(".context").join("graph.db");
    let store = SqliteGraphStore::open(&db_path).map_err(|e| anyhow::anyhow!("opening graph store at {}: {e}", db_path.display()))?;
    let repo = agentops_heavy_embeddings::repo_name(repo_path);

    let report = agentops_heavy_consolidate::consolidate_model(&store, &repo)?;

    if !report.attempted {
        return Ok(format!("Skipped: {}", report.reason));
    }

    Ok(format!(
        "{}\nexamples used: {}\ncandidate score: {:.3}\nbaseline score: {:.3}\npromoted: {}{}",
        report.reason,
        report.examples_used,
        report.candidate_score,
        report.baseline_score,
        report.promoted,
        report.promoted_version.map(|v| format!("\npromoted version: v{v}")).unwrap_or_default(),
    ))
}
