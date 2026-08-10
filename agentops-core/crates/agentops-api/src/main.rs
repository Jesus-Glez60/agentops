//! Standalone REST API binary — `agentops-api [--addr <host:port>] [--mode advisor|full]`.
//! Set `AGENTOPS_API_KEY_HASH` in the environment to require auth (see
//! `agentops_security::api_key::generate_api_key`).

use agentops_mcp::AccessMode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut addr = "127.0.0.1:8422".to_string();
    let mut mode = AccessMode::Advisor;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--addr" => {
                if let Some(v) = args.next() {
                    addr = v;
                }
            }
            "--mode" => match args.next().as_deref() {
                Some("full") => mode = AccessMode::Full,
                Some("advisor") => mode = AccessMode::Advisor,
                other => anyhow::bail!("invalid --mode value: {other:?}, expected advisor or full"),
            },
            other => anyhow::bail!("unknown argument '{other}'"),
        }
    }

    agentops_api::run(&addr, mode).await
}
