//! Driving (inbound) adapter: stdio MCP server for docbrain. Every tool
//! routes through the `DocbrainStore` port, so swapping the SQLite adapter
//! for a different one later wouldn't touch this layer.

mod protocol;
mod server;
mod tools;

pub use protocol::{CallToolResult, ContentBlock, ToolDefinition};
pub use tools::{call_tool, list_tools};

use std::io::{BufRead, Write};
use std::path::Path;

use docbrain_graph::SqliteDocbrainStore;

/// Runs the server over stdin/stdout until stdin closes, backed by a
/// docbrain store at `db_path` (created if it doesn't exist).
pub fn run_stdio(db_path: &Path) -> anyhow::Result<()> {
    let store = SqliteDocbrainStore::open(db_path)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server::handle_message(&store, db_path, &line) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }

    Ok(())
}
