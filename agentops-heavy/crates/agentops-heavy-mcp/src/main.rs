//! Server binary. Config via env vars: `AGENTOPS_QDRANT_URL` (required),
//! `AGENTOPS_QDRANT_COLLECTION` (optional, defaults to `agentops_semantic`).
//! No license requirement — this is a 1.0 (unpaid) release; `agentops-license`
//! stays untouched, a deliberate scope decision (see this crate's `lib.rs`
//! doc comment).
//!
//! Exposes four tools total: `semantic_search`/`semantic_index` (codebrain,
//! over an `agentops-graph` SQLite store) and `search_docs`/`index_docs`
//! (docbrain, over a `docbrain-graph` SQLite store) — both corpora share one
//! `SemanticIndex`/Qdrant collection (one BGE-M3 model load, not two) but
//! are kept from leaking into each other's results via a `kind` filter
//! (`"symbol"/"gotcha"/"decision"` vs `"doc"`) and a namespaced id space
//! (see `agentops_heavy_embeddings::collect_doc_index_items`'s doc comment).

use agentops_heavy_embeddings::SemanticIndex;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let qdrant_url = std::env::var("AGENTOPS_QDRANT_URL").map_err(|_| anyhow::anyhow!("AGENTOPS_QDRANT_URL must be set"))?;
    let collection = std::env::var("AGENTOPS_QDRANT_COLLECTION").unwrap_or_else(|_| "agentops_semantic".to_string());

    eprintln!("Loading BGE-M3 (downloads on first run, cached after)...");
    let index = SemanticIndex::connect(&qdrant_url, &collection)?;
    index.ensure_collection().await?;
    eprintln!("agentops-heavy-mcp ready (collection {collection:?} at {qdrant_url}).");

    agentops_heavy_mcp::run_stdio(index).await
}
