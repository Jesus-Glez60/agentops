//! Stdio MCP server for docbrain — same hand-rolled JSON-RPC approach and
//! rationale as `agentops-mcp`. Every tool routes through `docbrain-graph`'s
//! `TenantContext`-scoped API, so private-library isolation (Docbrain-4) holds
//! regardless of which transport (this stdio server, or `docbrain-api`) a
//! request came in through.

mod protocol;
mod server;
mod tools;

pub use protocol::{CallToolResult, ContentBlock, ToolDefinition};
pub use tools::{call_tool, list_tools};

use std::io::{BufRead, Write};
use std::path::Path;

use docbrain_graph::DocbrainStore;

/// Runs the server over stdin/stdout until stdin closes, backed by a
/// docbrain store at `db_path` (created if it doesn't exist).
pub fn run_stdio(db_path: &Path) -> anyhow::Result<()> {
    let store = DocbrainStore::open(db_path)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server::handle_message(&store, &line) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }

    Ok(())
}
