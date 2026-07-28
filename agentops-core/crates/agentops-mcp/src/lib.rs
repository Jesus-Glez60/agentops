//! Stdio MCP server wrapping the core crates. `AccessMode` structurally gates
//! which tools exist at all — `Advisor` mode's `tools/list` never includes the
//! write-capable tools, rather than relying on a prompt telling the model not
//! to call them (see the plan's §Security on this distinction).
//!
//! Hand-rolled JSON-RPC/MCP framing rather than the `rmcp` SDK crate — `rmcp`
//! is still pre-1.0 (beta as of this writing) and the protocol surface this
//! server actually needs (`initialize`, `tools/list`, `tools/call` over
//! newline-delimited JSON on stdio) is small enough to fully control and unit
//! test without an external MCP client.

mod protocol;
mod server;
mod tools;

pub use tools::AccessMode;

use std::io::{BufRead, Write};

/// Runs the server over stdin/stdout until stdin closes. One request per
/// line (newline-delimited JSON), matching the framing every MCP stdio
/// client already speaks.
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
