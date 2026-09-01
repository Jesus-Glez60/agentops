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

/// The docbrain store location — `AGENTOPS_DOCBRAIN_DB` if set, else
/// `$AGENTOPS_DATA_DIR/docbrain.db` (default `~/.agentops/docbrain.db`). One
/// canonical definition, called by this crate's own binary, `docbrain-api`,
/// `agentops-mcp-server`/`agentops-server`, and `agentops-cli`'s
/// `docbrain-serve`/`docbrain-serve-api` passthrough commands, rather than
/// each of them keeping its own private copy (two of which already existed
/// independently before this one was added). This is the single-tenant,
/// CLI-facing store — distinct from `agentops-heavy-api`'s per-tenant
/// `docbrain-tenants/` directory (`DOCBRAIN_DB_DIR`), which has its own
/// default under the same `AGENTOPS_DATA_DIR`.
pub fn default_db_path() -> PathBuf {
    if let Ok(path) = std::env::var("AGENTOPS_DOCBRAIN_DB") {
        return PathBuf::from(path);
    }
    agentops_manifest::agentops_data_dir().join("docbrain.db")
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
