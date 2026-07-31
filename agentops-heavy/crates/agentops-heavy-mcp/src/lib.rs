//! Stdio MCP server exposing semantic search (`semantic_search`/`semantic_index`)
//! as real callable tools — this is the piece that was missing for actual
//! agent use (Claude Code, etc.), as opposed to `agentops-heavy-api`'s REST
//! endpoints (dashboard use) or the `agentops-embeddings` CLI example.
//!
//! No `AccessMode`/advisor-mode concept here, unlike `agentops-mcp`: every
//! tool this server has is read/index-only (no code modification), so
//! there's no write-capable variant to structurally gate. The gate that
//! does apply is licensing — this whole server simply refuses to start
//! without a valid license (see `main.rs`), since semantic search is a
//! paid-tier capability.

mod protocol;
mod server;
mod tools;

pub use protocol::{CallToolResult, ContentBlock, ToolDefinition};
pub use tools::{call_tool, list_tools};

use std::io::{BufRead, Write};

use agentops_embeddings::SemanticIndex;

/// Runs the server over stdin/stdout until stdin closes. One request per
/// line (newline-delimited JSON), matching the framing every MCP stdio
/// client already speaks.
pub async fn run_stdio(mut index: SemanticIndex) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server::handle_message(&mut index, &line).await {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }

    Ok(())
}
