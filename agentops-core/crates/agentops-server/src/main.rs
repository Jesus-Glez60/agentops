//! Standalone binary wrapper — see `lib.rs` for what this actually does.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agentops_server::run().await
}
