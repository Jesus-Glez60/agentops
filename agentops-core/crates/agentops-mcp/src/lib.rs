//! Driving (inbound) adapter: MCP tool handlers, plus the use-case
//! functions they call. Handlers stay thin — see `scan.rs`'s
//! `scan_and_persist` for the actual scan-persistence use case, shared
//! identically by the CLI and the MCP `scan_repo` tool.

mod docgen;
mod init;
mod notes;
mod protocol;
pub mod scan;
mod server;
mod store;
mod tools;

pub use docgen::generate_docs;
pub use init::{init_agents_md, InitResult};
pub use notes::{add_note, ingest_notes_dir, AddNoteResult};
pub use protocol::{CallToolResult, ContentBlock, ToolAnnotations, ToolDefinition};
pub use scan::{graph_db_path, persist, repo_name, scan_and_persist, ScanPersistSummary};
pub use store::{describe_backend, open_store};
pub use tools::{call_tool, list_tools, AccessMode};

use std::io::{BufRead, Write};

/// Runs the stdio MCP server until stdin closes. `mode` is fixed for the
/// whole process — set once at startup (a CLI flag), not negotiated
/// per-request.
pub fn run_stdio(mode: AccessMode) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server::handle_message(mode, &line) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }

    Ok(())
}
