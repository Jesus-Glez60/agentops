//! Driving (inbound) adapter: stdio MCP server for docbrain. Every tool
//! routes through the `DocbrainStore` port, so swapping the SQLite adapter
//! for a different one later wouldn't touch this layer.

mod protocol;
mod server;
mod tools;

pub use protocol::{CallToolResult, ContentBlock, ToolDefinition};
pub use tools::{call_tool, list_tools};

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use docbrain_graph::SqliteDocbrainStore;

/// The default docbrain store location — `~/.agentops/docbrain.db`. One
/// canonical definition, called by this crate's own binary, `docbrain-api`,
/// and `agentops-cli`'s `docbrain-serve`/`docbrain-serve-api` passthrough
/// commands, rather than each of them keeping its own private copy (two of
/// which already existed independently before this one was added).
pub fn default_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agentops").join("docbrain.db")
}

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
