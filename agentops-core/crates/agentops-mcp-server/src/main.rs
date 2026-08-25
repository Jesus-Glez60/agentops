//! Unified stdio MCP server binary. Config via env vars: `AGENTOPS_ACCESS_MODE`
//! (`advisor`|`full`, default `advisor` — matches `agentops-mcp`'s own CLI
//! default), `AGENTOPS_DOCBRAIN_DB`/`AGENTOPS_DATA_DIR` (see
//! `docbrain_mcp::default_db_path`'s doc comment), and
//! `AGENTOPS_QDRANT_URL`/`AGENTOPS_QDRANT_COLLECTION` (optional — enables
//! `agentops-heavy-mcp`'s tools; loading the BGE-M3 model can take real time
//! on first run, downloading it if not already cached).

use agentops_mcp::AccessMode;
use agentops_mcp_server::Dispatch;
use docbrain_graph::SqliteDocbrainStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mode = match std::env::var("AGENTOPS_ACCESS_MODE").as_deref() {
        Ok("full") => AccessMode::Full,
        _ => AccessMode::Advisor,
    };

    let docbrain_db_path = docbrain_mcp::default_db_path();
    let docbrain_store = SqliteDocbrainStore::open(&docbrain_db_path)?;

    let heavy_index = match std::env::var("AGENTOPS_QDRANT_URL") {
        Ok(qdrant_url) => {
            eprintln!("AGENTOPS_QDRANT_URL set — loading BGE-M3 (downloads on first run, cached after)...");
            let collection = std::env::var("AGENTOPS_QDRANT_COLLECTION").unwrap_or_else(|_| "agentops_semantic".to_string());
            let index = agentops_heavy_embeddings::SemanticIndex::connect(&qdrant_url, &collection)?;
            index.ensure_collection().await?;
            eprintln!("agentops-mcp-server: heavy tools ready (collection {collection:?} at {qdrant_url}).");
            Some(index)
        }
        Err(_) => {
            eprintln!("AGENTOPS_QDRANT_URL not set — semantic_search/semantic_index/search_docs_indexed/index_docs_indexed/consolidate_model unavailable.");
            None
        }
    };

    let dispatch = Dispatch { mode, docbrain_store, docbrain_db_path, heavy_index };
    agentops_mcp_server::run_stdio(dispatch).await
}
