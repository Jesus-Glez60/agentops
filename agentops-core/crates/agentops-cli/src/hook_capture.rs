//! `agentops hook-capture` — invoked as a Claude Code `PostToolUse` hook
//! (see `write_claude_post_tool_use_hook` in `main.rs`). Reads the hook's JSON
//! payload from stdin, and if the tool's own output is large enough that
//! inlining it into context would be wasteful, redacts it and persists it in
//! full as both a `session_events` row (for the session timeline)
//! and a `NodeKind::ToolOutput` node (so it's searchable via
//! `semantic_search`'s hybrid/lexical mode and retrievable in full via
//! `fetch_content` — the same "search instead of re-transmit" payoff
//! Context Mode's own hook-based interception gets, built on infrastructure
//! this repo already has rather than a new store).
//!
//! Phase 1, deliberately narrow: `PostToolUse` only (passive capture, no
//! blocking/redirecting), Claude Code only. See the plan doc this was
//! implemented from for what's explicitly deferred.

use std::io::Read;

use agentops_graph::{NewNode, NodeKind};
use agentops_mcp::budget::DEFAULT_CHAR_BUDGET;
use anyhow::{Context, Result};
use serde::Deserialize;

/// Claude Code's `PostToolUse` hook payload — only the fields this hook
/// actually uses; every other field Claude Code sends is ignored via serde's
/// default "unknown fields are fine" behavior (no `deny_unknown_fields`).
#[derive(Debug, Deserialize)]
struct PostToolUsePayload {
    tool_name: String,
    #[serde(default)]
    tool_response: serde_json::Value,
    session_id: Option<String>,
    cwd: Option<String>,
}

/// Entry point for the `hook-capture` subcommand. A `PostToolUse` hook's
/// own nonzero exit doesn't block or mutate the tool call it's reacting to
/// (unlike `PreToolUse`) -- Claude Code just logs the failure -- so
/// returning `Err` here is safe; it never needs to be swallowed.
pub fn run() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).context("reading hook payload from stdin")?;
    process(&input)
}

/// The actual logic, taking the payload as a string rather than reading
/// stdin directly so it's callable from a test without a subprocess.
fn process(input: &str) -> Result<()> {
    let payload: PostToolUsePayload = serde_json::from_str(input).context("parsing PostToolUse payload")?;

    let text = render_tool_response(&payload.tool_response);
    if text.chars().count() <= DEFAULT_CHAR_BUDGET {
        // Small enough to not be worth capturing — the whole point is only
        // ever intervening on the outputs that would otherwise bloat context.
        return Ok(());
    }

    let Some(cwd) = payload.cwd.as_deref() else {
        return Ok(());
    };
    let repo_path = std::path::Path::new(cwd);
    let store = agentops_mcp::open_store(repo_path)?;
    let repo = agentops_mcp::repo_name(repo_path);

    // Stored in full (redacted, not truncated) — the whole point of a
    // Node-backed capture is that `fetch_content`/lexical search can reach
    // *all* of it later; only the inline tool responses in `agentops-mcp`
    // (get_symbol, related_context, explain_symbol) cap what they return.
    let redaction = agentops_security::redact(&text);

    let session_id = payload.session_id.as_deref().unwrap_or("unknown-session");
    let node_id = store.add_node(NewNode {
        kind: NodeKind::ToolOutput,
        repo: repo.clone(),
        path: Some(format!("tool_output:{session_id}:{}", now_nanos())),
        name: Some(payload.tool_name.clone()),
        container: None,
        start_line: None,
        end_line: None,
        content: Some(redaction.text),
    })?;

    store.record_session_event(&repo, session_id, &payload.tool_name, &format!("captured {} chars of tool output ({} secret(s) redacted)", text.chars().count(), redaction.redacted_count), Some(node_id), "tool_output")?;

    Ok(())
}

/// `tool_response` is whatever shape the underlying tool returned — a plain
/// string for most shell-like tools, but not guaranteed. Renders it to text
/// either way rather than assuming a shape and erroring on anything else.
fn render_tool_response(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Nanosecond timestamp used only to keep each captured event's `path`
/// unique (see `upsert_node`'s natural-key doc comment — `add_node` is used
/// directly here instead, since each hook-capture event is a distinct
/// occurrence to record, never one to update in place).
fn now_nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_tool_response_passes_through_a_plain_string() {
        assert_eq!(render_tool_response(&serde_json::json!("hello")), "hello");
    }

    #[test]
    fn render_tool_response_stringifies_a_non_string_shape() {
        assert_eq!(render_tool_response(&serde_json::json!({"a": 1})), r#"{"a":1}"#);
    }

    #[test]
    fn small_tool_output_is_not_captured() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Bash", "tool_response": "short output", "session_id": "sess-1", "cwd": path }).to_string();

        process(&payload).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        assert!(store.nodes_by_kind(&repo, NodeKind::ToolOutput).unwrap().is_empty(), "small output should not be captured as a node");
    }

    #[test]
    fn large_tool_output_is_captured_redacted_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let big_output = format!("{}\nAWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLE", "line of shell output\n".repeat(300));
        let payload = serde_json::json!({ "tool_name": "Bash", "tool_response": big_output, "session_id": "sess-2", "cwd": path }).to_string();

        process(&payload).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let nodes = store.nodes_by_kind(&repo, NodeKind::ToolOutput).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name.as_deref(), Some("Bash"));
        let content = nodes[0].content.as_deref().unwrap();
        assert!(content.contains("[REDACTED:unquoted-env-credential-assignment]"), "{content}");
        assert!(!content.contains("wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLE"));
        assert!(content.contains("line of shell output"), "content should be stored in full, not truncated");

        let events = store.session_events(&repo, "sess-2").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tool_name, "Bash");
    }
}
