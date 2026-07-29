//! Server binary for the heavy-tier repo-access API. `agentops-cli` (the
//! open-core CLI in the root workspace) can't depend on this — pulling in
//! `agentops-heavy` code would ship commercially-licensed code inside the
//! MIT/Apache-licensed CLI binary — so this is its own small binary instead.
//!
//! Config via env vars: `AGENTOPS_HEAVY_API_ADDR` (default `127.0.0.1:8978`),
//! `AGENTOPS_HEAVY_API_DB` (default `./agentops-heavy-api.sqlite`), plus
//! whatever `agentops_heavy_api::run` itself reads (`AGENTOPS_SECRETS_MASTER_KEY`,
//! required; `AGENTOPS_HEAVY_API_KEY_HASH`, `AGENTOPS_GITHUB_APP_SLUG`, optional).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = std::env::var("AGENTOPS_HEAVY_API_ADDR").unwrap_or_else(|_| "127.0.0.1:8978".to_string());
    let db_path = std::env::var("AGENTOPS_HEAVY_API_DB").unwrap_or_else(|_| "./agentops-heavy-api.sqlite".to_string());
    agentops_heavy_api::run(&addr, std::path::Path::new(&db_path)).await
}
