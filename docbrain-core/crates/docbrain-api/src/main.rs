//! Standalone REST API binary — `docbrain-api [--addr <host:port>] [--db <path>]`.
//! Set `DOCBRAIN_API_KEY_HASH` in the environment to require auth (see
//! `agentops_security::api_key::generate_api_key`).

use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut addr = "127.0.0.1:8421".to_string();
    let mut db_path = docbrain_mcp::default_db_path();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--addr" => {
                if let Some(v) = args.next() {
                    addr = v;
                }
            }
            "--db" => {
                if let Some(v) = args.next() {
                    db_path = PathBuf::from(v);
                }
            }
            other => anyhow::bail!("unknown argument '{other}'"),
        }
    }

    docbrain_api::run(&addr, &db_path).await
}
