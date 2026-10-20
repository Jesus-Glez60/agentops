//! Standalone binary wrapper — see `lib.rs` for what this actually does.
//!
//! One maintenance subcommand runs instead of the server:
//! `agentops-server link-similar <repo> [--prune]` is the similarity backfill
//! (milestone 2, ADR 0051), run inside the deployed app so it uses that
//! deployment's own store settings (`AGENTOPS_DATABASE_URL`) and no
//! credentials are handled by hand. `<repo>` is the store's repo name,
//! `<connection_id>--<tenant>` on the hosted server. It runs before any
//! Tokio runtime exists: the Postgres store must never be opened on a
//! runtime thread (gotcha 54054).

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("link-similar") {
        let repo = args.get(1).filter(|a| !a.starts_with("--")).ok_or_else(|| anyhow::anyhow!("usage: agentops-server link-similar <repo> [--prune]"))?;
        let prune = args.iter().skip(2).any(|a| a == "--prune");
        let store = agentops_mcp::open_store(std::path::Path::new("."))?;
        print!("{}", agentops_mcp::link_similar_backfill(store.as_ref(), repo, prune)?);
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(agentops_server::run())
}
