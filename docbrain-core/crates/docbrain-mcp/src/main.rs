//! Standalone stdio MCP server binary — `docbrain-mcp [--db <path>]`.
//! Talks JSON-RPC over stdin/stdout, one line per message, per the MCP
//! stdio transport convention.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let db_path = parse_db_arg().unwrap_or_else(default_db_path);
    docbrain_mcp::run_stdio(&db_path)
}

fn parse_db_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--db" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

fn default_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agentops").join("docbrain.db")
}
