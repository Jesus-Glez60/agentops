//! Server binary. Config via env vars: `AGENTOPS_QDRANT_URL` (required),
//! `AGENTOPS_LICENSE_KEY` (required — semantic search is a paid-tier
//! feature; this binary refuses to start without a valid one, rather than
//! starting in some degraded no-tools mode — an MCP server with zero tools
//! isn't a useful thing to hand an agent), `AGENTOPS_QDRANT_COLLECTION`
//! (optional, defaults to `agentops_semantic`).

use agentops_embeddings::SemanticIndex;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let claims = agentops_license::require_valid_license_from_env()
        .map_err(|e| anyhow::anyhow!("no valid license found ({e}) — semantic search is a paid-tier feature; set AGENTOPS_LICENSE_KEY"))?;
    eprintln!("Licensed to {:?} (tier: {:?}).", claims.licensee, claims.tier);

    let qdrant_url = std::env::var("AGENTOPS_QDRANT_URL").map_err(|_| anyhow::anyhow!("AGENTOPS_QDRANT_URL must be set"))?;
    let collection = std::env::var("AGENTOPS_QDRANT_COLLECTION").unwrap_or_else(|_| "agentops_semantic".to_string());

    eprintln!("Loading BGE-M3 (downloads on first run, cached after)...");
    let index = SemanticIndex::connect(&qdrant_url, &collection)?;
    index.ensure_collection().await?;
    eprintln!("agentops-heavy-mcp ready (collection {collection:?} at {qdrant_url}).");

    agentops_heavy_mcp::run_stdio(index).await
}
