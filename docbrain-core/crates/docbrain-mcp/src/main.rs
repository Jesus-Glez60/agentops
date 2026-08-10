//! Standalone stdio MCP server binary — `docbrain-mcp [--db <path>]`.
//! Talks JSON-RPC over stdin/stdout, one line per message, per the MCP
//! stdio transport convention.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let db_path = parse_db_arg().unwrap_or_else(docbrain_mcp::default_db_path);
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
