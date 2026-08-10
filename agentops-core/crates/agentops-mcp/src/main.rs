//! Standalone stdio MCP server binary — `agentops-mcp [--mode advisor|full]`.
//! Talks JSON-RPC over stdin/stdout, one line per message, per the MCP
//! stdio transport convention. Unlike `docbrain-mcp`, there is no `--db`
//! flag: every tool call carries its own repo `path` in its arguments, so
//! one process can serve requests about many different repos.

use agentops_mcp::AccessMode;

fn main() -> anyhow::Result<()> {
    let mode = parse_mode_arg().unwrap_or(AccessMode::Advisor);
    agentops_mcp::run_stdio(mode)
}

fn parse_mode_arg() -> Option<AccessMode> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--mode" {
            return match args.next().as_deref() {
                Some("full") => Some(AccessMode::Full),
                Some("advisor") => Some(AccessMode::Advisor),
                _ => None,
            };
        }
    }
    None
}
