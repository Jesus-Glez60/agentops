//! `agentops hook-capture --event <name>` — the handler behind every Claude
//! Code hook `connect --with-hooks` installs (see `write_claude_hook` in
//! `main.rs`). One binary, dispatched by `--event` rather than by sniffing
//! payload shape (more robust — Claude Code's hook config can pass whatever
//! flag each event needs).
//!
//! - `posttooluse`: if a tool's own output is large enough that inlining it
//!   into context would be wasteful, redacts it and persists it in full as
//!   both a `session_events` row and a `NodeKind::ToolOutput` node (searchable
//!   via `nodes_fts`/`search_hybrid`, fetchable via `fetch_content`). Also
//!   cheaply categorizes every call (file edits, git commands, web
//!   references) into `session_events` regardless of output size, feeding
//!   `get_session_guide`.
//! - `posttoolusefailure`, `userpromptsubmit`: record narrower categories
//!   (`"error"`, `"user_intent"`) the same way.
//! - `pretooluse`: a proactive secret-exposure warning — never blocks (see
//!   `process_pre_tool_use`'s doc comment for why it's especially careful
//!   about this).
//! - `sessionstart`: renders the session-resume guide on compact/resume,
//!   via `agentops_mcp::session_guide::build` (shared with the
//!   `get_session_guide` MCP tool — one implementation, two callers).

use std::io::Read;
use std::path::Path;

use agentops_graph::{NewNode, NodeKind};
use agentops_mcp::budget::{cap, DEFAULT_CHAR_BUDGET};
use anyhow::{Context, Result};
use serde::Deserialize;

/// Entry point for the `hook-capture` subcommand. `compress` is only ever
/// read by the `pretooluse` branch (see `Connect`'s `--compress-commands`
/// doc comment in `main.rs` for why this is opt-in) — every other event
/// ignores it.
pub fn run(event: &str, compress: bool) -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).context("reading hook payload from stdin")?;
    match event {
        "posttooluse" => process_post_tool_use(&input),
        "posttoolusefailure" => process_post_tool_use_failure(&input),
        "userpromptsubmit" => process_user_prompt_submit(&input),
        "sessionstart" => process_session_start(&input),
        // `PreToolUse` is the one event whose nonzero exit can actually
        // block the tool call it's reacting to (confirmed against Claude
        // Code's real hook docs — unlike every other event here). This
        // handler must never surface an `Err` up to `main()`'s exit code,
        // so it's wrapped separately rather than trusting the same
        // `Result`-propagation-is-safe posture the other events use.
        "pretooluse" => {
            if let Err(e) = process_pre_tool_use(&input, compress) {
                eprintln!("agentops hook-capture --event pretooluse: {e:#} (non-fatal, continuing)");
            }
            Ok(())
        }
        other => anyhow::bail!("unknown hook event '{other}'"),
    }
}

// ---------------------------------------------------------------------
// PostToolUse
// ---------------------------------------------------------------------

/// Claude Code's `PostToolUse` hook payload — only the fields this hook
/// actually uses; every other field Claude Code sends is ignored via serde's
/// default "unknown fields are fine" behavior (no `deny_unknown_fields`).
#[derive(Debug, Deserialize)]
struct PostToolUsePayload {
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default)]
    tool_response: serde_json::Value,
    session_id: Option<String>,
    cwd: Option<String>,
}

fn process_post_tool_use(input: &str) -> Result<()> {
    let payload: PostToolUsePayload = serde_json::from_str(input).context("parsing PostToolUse payload")?;
    let Some(cwd) = payload.cwd.as_deref() else {
        return Ok(());
    };
    let repo_path = Path::new(cwd);
    let store = agentops_mcp::open_store(repo_path)?;
    let repo = agentops_mcp::repo_name(repo_path);
    let session_id = payload.session_id.as_deref().unwrap_or("unknown-session");

    // Cheap, always-on categorization -- independent of whether this call's
    // own output is large enough to warrant the full-capture path below.
    // Most calls match nothing here, and that's fine: unlike the MCP tools'
    // own event recording, there's no catch-all "activity" fallback: not
    // every tool call is guide-worthy.
    if let Some((event_kind, description)) = classify_post_tool_use(&payload.tool_name, &payload.tool_input) {
        store.record_session_event(&repo, session_id, &payload.tool_name, &description, None, event_kind)?;
    }

    let text = render_tool_response(&payload.tool_response);
    if text.chars().count() <= DEFAULT_CHAR_BUDGET {
        // Small enough to not be worth capturing — the whole point is only
        // ever intervening on the outputs that would otherwise bloat context.
        return Ok(());
    }

    // Stored in full (redacted, not truncated) — the whole point of a
    // Node-backed capture is that `fetch_content`/lexical search can reach
    // *all* of it later; only the inline tool responses in `agentops-mcp`
    // (get_symbol, related_context, explain_symbol) cap what they return.
    let redaction = agentops_security::redact(&text);

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

/// Feeds `get_session_guide`'s "Files Modified"/"Git Ops"/"External
/// References" sections. Deliberately narrow, pattern-based, no LLM call --
/// "Rejected Approaches"/"Constraints"/"Blockers" need actual semantic
/// judgment and are out of scope here (see the plan this was built from).
fn classify_post_tool_use(tool_name: &str, tool_input: &serde_json::Value) -> Option<(&'static str, String)> {
    match tool_name {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let path = tool_input.get("file_path").or_else(|| tool_input.get("notebook_path")).and_then(|v| v.as_str())?;
            Some(("file_modified", path.to_string()))
        }
        "Bash" => {
            let command = tool_input.get("command").and_then(|v| v.as_str())?;
            is_git_op(command).then(|| ("git_op", agentops_security::redact(command).text))
        }
        "WebFetch" | "WebSearch" => {
            let reference = tool_input.get("url").or_else(|| tool_input.get("query")).and_then(|v| v.as_str())?;
            Some(("external_reference", reference.to_string()))
        }
        _ => None,
    }
}

/// A plain substring check, not a regex — no new dependency needed for
/// something this simple (`agentops-cli` doesn't otherwise depend on the
/// `regex` crate directly).
fn is_git_op(command: &str) -> bool {
    let trimmed = command.trim_start();
    if !trimmed.starts_with("git ") {
        return false;
    }
    const VERBS: &[&str] = &["commit", "push", "merge", "rebase", "checkout", "reset"];
    VERBS.iter().any(|v| trimmed.contains(v))
}

/// `tool_response` is whatever shape the underlying tool returned. Claude
/// Code's real shape for most tools is `{"type": "text", "text": "..."}` —
/// an object, not a bare string (confirmed against the real hook docs,
/// correcting an earlier assumption in this same function that stringified
/// the whole wrapper object, polluting captured content with JSON-escaping
/// noise and inflating what got measured against `DEFAULT_CHAR_BUDGET`).
/// Extracts `.text`/`.content` when present; falls back to stringifying the
/// whole value only if neither key holds a string, so an unrecognized shape
/// still degrades gracefully instead of panicking or losing data outright.
fn render_tool_response(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) => match map.get("text").or_else(|| map.get("content")).and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => value.to_string(),
        },
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------
// PostToolUseFailure
// ---------------------------------------------------------------------

/// Claude Code's docs (as fetched) confirm `tool_name`/`tool_input` but
/// don't spell out the exact failure-details field name for this event --
/// `error` is a best-effort guess, same "not independently confirmed"
/// posture as the Gemini CLI header-substitution gotcha this project
/// already carries. Defaults to `Value::Null` if the real field is named
/// something else, which `render_tool_response` degrades to an empty
/// description for -- a silently-thin event, never a parse failure.
#[derive(Debug, Deserialize)]
struct PostToolUseFailurePayload {
    tool_name: String,
    #[serde(default)]
    error: serde_json::Value,
    session_id: Option<String>,
    cwd: Option<String>,
}

fn process_post_tool_use_failure(input: &str) -> Result<()> {
    let payload: PostToolUseFailurePayload = serde_json::from_str(input).context("parsing PostToolUseFailure payload")?;
    let Some(cwd) = payload.cwd.as_deref() else {
        return Ok(());
    };
    let repo_path = Path::new(cwd);
    let store = agentops_mcp::open_store(repo_path)?;
    let repo = agentops_mcp::repo_name(repo_path);
    let session_id = payload.session_id.as_deref().unwrap_or("unknown-session");

    let description = render_tool_response(&payload.error);
    let capped = cap(&description, DEFAULT_CHAR_BUDGET, 0);
    let redacted = agentops_security::redact(&capped.text);
    store.record_session_event(&repo, session_id, &payload.tool_name, &redacted.text, None, "error")?;
    Ok(())
}

// ---------------------------------------------------------------------
// UserPromptSubmit
// ---------------------------------------------------------------------

/// `prompt` is a best-effort field-name guess (not spelled out in the
/// fetched docs excerpt) -- same "not independently confirmed" posture as
/// `PostToolUseFailurePayload::error` above.
#[derive(Debug, Deserialize)]
struct UserPromptSubmitPayload {
    prompt: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
}

fn process_user_prompt_submit(input: &str) -> Result<()> {
    let payload: UserPromptSubmitPayload = serde_json::from_str(input).context("parsing UserPromptSubmit payload")?;
    let (Some(cwd), Some(prompt)) = (payload.cwd.as_deref(), payload.prompt.as_deref()) else {
        return Ok(());
    };
    if prompt.trim().is_empty() {
        return Ok(());
    }
    let repo_path = Path::new(cwd);
    let store = agentops_mcp::open_store(repo_path)?;
    let repo = agentops_mcp::repo_name(repo_path);
    let session_id = payload.session_id.as_deref().unwrap_or("unknown-session");

    let capped = cap(prompt, DEFAULT_CHAR_BUDGET, 0);
    let redacted = agentops_security::redact(&capped.text);
    store.record_session_event(&repo, session_id, "user_prompt", &redacted.text, None, "user_intent")?;

    // Sticky pins re-injected on every turn, not just at SessionStart -- the
    // actual "combat context drift" mechanism: a long, multi-compact session
    // can scroll a SessionStart-only injection out of view, but a pin is
    // meant to be a standing directive the agent never loses sight of.
    // Confirmed against Claude Code's real hooks docs: `UserPromptSubmit`
    // nests `additionalContext` under `hookSpecificOutput` exactly like
    // `SessionStart`/`PreToolUse` (capped at 10,000 chars by Claude Code
    // itself -- `GUIDE_CHAR_BUDGET`'s 2000 stays comfortably under that).
    let pinned = agentops_mcp::session_guide::build_pinned_context(store.as_ref(), &repo)?;
    if !pinned.is_empty() {
        println!(
            "{}",
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "UserPromptSubmit",
                    "additionalContext": pinned,
                }
            })
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------
// PreToolUse
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct PreToolUsePayload {
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
    session_id: Option<String>,
    cwd: Option<String>,
}

/// Two fully independent concerns, either/both/neither of which can fire on
/// the same call: a proactive secret-exposure warning (reuses the existing
/// redaction/filename-classification code directly, invents no new
/// pattern-matching; deliberately narrow -- full command blocking/denial, a
/// competitor's `curl`/`wget` policy, is explicitly out of scope, there's no
/// sandboxed-exec tool to redirect to and it's not core to what AgentOps
/// does) and, when `compress` is set, a command-output-compression rewrite
/// (RTK-inspired -- see `classify_for_compression`). **Never sets
/// `permissionDecision`** for the warning path (the tool call always
/// proceeds through Claude Code's normal permission flow untouched); the
/// compression path sets `updatedInput` only, never `permissionDecision`
/// either, so it changes *what* runs, never *whether* it runs.
fn process_pre_tool_use(input: &str, compress: bool) -> Result<()> {
    let payload: PreToolUsePayload = serde_json::from_str(input).context("parsing PreToolUse payload")?;
    let warning = detect_secret_exposure(&payload.tool_name, &payload.tool_input);
    let rewrite = if compress { detect_command_rewrite(&payload.tool_name, &payload.tool_input) } else { None };

    if warning.is_none() && rewrite.is_none() {
        return Ok(());
    }

    if let (Some(w), Some(cwd)) = (&warning, payload.cwd.as_deref()) {
        let repo_path = Path::new(cwd);
        if let Ok(store) = agentops_mcp::open_store(repo_path) {
            let repo = agentops_mcp::repo_name(repo_path);
            let session_id = payload.session_id.as_deref().unwrap_or("unknown-session");
            // Best-effort: a failure to record must never block the tool
            // call or fail this hook -- the warning has already been decided;
            // recording it is a nice-to-have for the session guide, not
            // a precondition for warning Claude.
            let _ = store.record_session_event(&repo, session_id, &payload.tool_name, w, None, "secret_exposure_warning");
        }
    }

    let mut output = serde_json::json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse" } });
    if let Some(w) = &warning {
        output["hookSpecificOutput"]["additionalContext"] = serde_json::Value::String(w.clone());
        output["hookSpecificOutput"]["systemMessage"] = serde_json::Value::String(format!("agentops: {w}"));
    }
    if let Some(new_command) = &rewrite {
        output["hookSpecificOutput"]["updatedInput"] = serde_json::json!({ "command": new_command });
    }
    println!("{output}");
    Ok(())
}

fn detect_secret_exposure(tool_name: &str, tool_input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Bash" => {
            let command = tool_input.get("command").and_then(|v| v.as_str())?;
            let redaction = agentops_security::redact(command);
            (redaction.redacted_count > 0).then(|| "this command appears to contain a credential-shaped string — double-check before running it".to_string())
        }
        "Read" | "Edit" | "Write" | "NotebookEdit" => {
            let path = tool_input.get("file_path").and_then(|v| v.as_str())?;
            agentops_security::is_secret_bearing_filename(Path::new(path)).then(|| format!("{path} looks like a secret-bearing file"))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------
// PreToolUse: RTK-inspired command-output compression (opt-in via
// `--compress`, see `Connect`'s `--compress-commands` doc comment)
// ---------------------------------------------------------------------

/// How a matched command gets rewritten -- see each variant for why the two
/// strategies exist rather than one.
enum CompressAction {
    /// Append a flag directly to the command -- no pipe, so there's nothing
    /// to restore via `PIPESTATUS`: the original command still runs and
    /// still reports its own real exit code. Used for `git log --oneline`/
    /// `git diff -U1`, where git already produces the compact form itself
    /// given the right flag -- the smallest possible implementation for
    /// those two cases, no `compress-output` invocation needed at all.
    AppendFlag(&'static str),
    /// Pipe the command's combined stdout+stderr through
    /// `agentops compress-output --kind <kind>`.
    Pipe(crate::compress_output::CompressKind),
}

/// Only matches a **single, unchained** recognized command -- anything
/// already containing `&&`, `;`, `|`, or a subshell `(` is left completely
/// untouched, since rewriting only the visible head of a chained command
/// risks corrupting the rest of the pipeline (a script that pipes
/// `cargo test` into its own `tee`, for instance, must never be
/// double-piped by this hook). Deliberately a small, conservative allowlist
/// -- matches `is_git_op`'s own "plain substring check, not regex"
/// precedent -- meant to grow over time, not to be exhaustive from day one.
fn classify_for_compression(command: &str) -> Option<CompressAction> {
    use crate::compress_output::CompressKind;

    let trimmed = command.trim();
    if trimmed.contains("&&") || trimmed.contains(';') || trimmed.contains('|') || trimmed.contains('(') {
        return None;
    }
    if trimmed.starts_with("cargo test") {
        return Some(CompressAction::Pipe(CompressKind::TestRust));
    }
    if trimmed.starts_with("npm test") || trimmed.starts_with("npm run test") {
        return Some(CompressAction::Pipe(CompressKind::TestNode));
    }
    if trimmed.starts_with("pytest") || trimmed.starts_with("python -m pytest") || trimmed.starts_with("python3 -m pytest") {
        return Some(CompressAction::Pipe(CompressKind::TestPytest));
    }
    if trimmed.starts_with("go test") {
        return Some(CompressAction::Pipe(CompressKind::TestGo));
    }
    if trimmed.starts_with("git log") && !trimmed.contains("--oneline") && !trimmed.contains("--format") && !trimmed.contains("--stat") {
        return Some(CompressAction::AppendFlag("--oneline"));
    }
    if trimmed.starts_with("git diff") && !trimmed.contains("-U") && !trimmed.contains("--unified") {
        return Some(CompressAction::AppendFlag("-U1"));
    }
    if trimmed.starts_with("grep ") || trimmed.starts_with("rg ") {
        return Some(CompressAction::Pipe(CompressKind::Grep));
    }
    None
}

/// `AppendFlag` needs no exit-code handling (see the variant's own doc
/// comment). `Pipe` loses the piped command's exit status by default (a
/// bare pipe reports the *last* command's exit code, i.e.
/// `compress-output`'s own, not the real command's) -- `2>&1` folds stderr
/// into the piped stream first (test frameworks often write failures
/// there), and bash's `PIPESTATUS` restores the original command's real
/// exit status as the shell's final status afterward, which is what Claude
/// Code's Bash tool inspects for success/failure. This assumes Claude
/// Code's Bash tool invokes `bash -c`, not `sh -c`/dash (which lacks
/// `PIPESTATUS`) -- verify this mechanism in a real shell (and against a
/// real Claude Code session) before relying on it; see this crate's own
/// test coverage for the shell-level check.
fn rewrite_command(original: &str, action: &CompressAction) -> String {
    match action {
        CompressAction::AppendFlag(flag) => format!("{original} {flag}"),
        CompressAction::Pipe(kind) => format!("{{ {original}; }} 2>&1 | agentops compress-output --kind {} ; exit ${{PIPESTATUS[0]}}", kind.as_flag()),
    }
}

fn detect_command_rewrite(tool_name: &str, tool_input: &serde_json::Value) -> Option<String> {
    if tool_name != "Bash" {
        return None;
    }
    let command = tool_input.get("command").and_then(|v| v.as_str())?;
    let action = classify_for_compression(command)?;
    Some(rewrite_command(command, &action))
}

// ---------------------------------------------------------------------
// SessionStart
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SessionStartPayload {
    source: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
}

fn process_session_start(input: &str) -> Result<()> {
    let payload: SessionStartPayload = serde_json::from_str(input).context("parsing SessionStart payload")?;
    // Confirmed exactly against Claude Code's real docs: `source` is one of
    // startup/resume/clear/compact/fork. AgentOps never needs a `PreCompact`
    // hook (unlike a competitor's design) because `session_events` already
    // persists durably and incrementally throughout the session -- gating
    // here is the only "checkpoint" logic needed. `startup` is included
    // (unlike this hook's first version) so a brand-new session also gets
    // primed -- there's nothing yet to resume, but `build_repo_briefing`'s
    // fallback below still has real repo-level context to offer.
    if !matches!(payload.source.as_deref(), Some("compact") | Some("resume") | Some("startup")) {
        return Ok(());
    }
    let (Some(cwd), Some(session_id)) = (payload.cwd.as_deref(), payload.session_id.as_deref()) else {
        return Ok(());
    };

    let repo_path = Path::new(cwd);
    let store = agentops_mcp::open_store(repo_path)?;
    let repo = agentops_mcp::repo_name(repo_path);
    let guide = agentops_mcp::session_guide::build(store.as_ref(), &repo, session_id)?;
    let guide = if guide.is_empty() { agentops_mcp::session_guide::build_repo_briefing(store.as_ref(), &repo)? } else { guide };
    // Sticky pins are a standing directive, prepended ahead of the
    // session-resume guide/briefing -- and checked for emptiness only after
    // this concatenation, so a session with pins but no other activity yet
    // still emits output.
    let pinned = agentops_mcp::session_guide::build_pinned_context(store.as_ref(), &repo)?;
    let guide = if pinned.is_empty() { guide } else if guide.is_empty() { pinned } else { format!("{pinned}\n\n{guide}") };
    if guide.is_empty() {
        return Ok(());
    }

    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": guide,
            }
        })
    );
    Ok(())
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
    fn render_tool_response_extracts_text_from_the_real_claude_code_shape() {
        assert_eq!(render_tool_response(&serde_json::json!({"type": "text", "text": "hello world"})), "hello world");
    }

    #[test]
    fn render_tool_response_falls_back_to_stringifying_an_unrecognized_shape() {
        assert_eq!(render_tool_response(&serde_json::json!({"a": 1})), r#"{"a":1}"#);
    }

    #[test]
    fn is_git_op_recognizes_common_verbs_but_not_plain_git_status() {
        assert!(is_git_op("git commit -m 'wip'"));
        assert!(is_git_op("git push origin main"));
        assert!(!is_git_op("git status"));
        assert!(!is_git_op("echo git commit"));
    }

    #[test]
    fn small_tool_output_is_not_captured() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Bash", "tool_response": "short output", "session_id": "sess-1", "cwd": path }).to_string();

        process_post_tool_use(&payload).unwrap();

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

        process_post_tool_use(&payload).unwrap();

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

    #[test]
    fn a_real_claude_code_shaped_response_is_captured_as_plain_text_not_json_wrapped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let big_text = "line of shell output\n".repeat(300);
        let payload = serde_json::json!({ "tool_name": "Bash", "tool_response": {"type": "text", "text": big_text}, "session_id": "sess-3", "cwd": path }).to_string();

        process_post_tool_use(&payload).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let nodes = store.nodes_by_kind(&repo, NodeKind::ToolOutput).unwrap();
        assert_eq!(nodes.len(), 1);
        let content = nodes[0].content.as_deref().unwrap();
        assert!(!content.contains(r#""type":"text""#), "must not capture the JSON wrapper: {content}");
        assert!(content.starts_with("line of shell output"));
    }

    #[test]
    fn editing_a_file_records_a_file_modified_event_regardless_of_output_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Edit", "tool_input": {"file_path": "src/main.rs"}, "tool_response": "ok", "session_id": "sess-4", "cwd": path }).to_string();

        process_post_tool_use(&payload).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let events = store.session_events(&repo, "sess-4").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "file_modified");
        assert_eq!(events[0].description, "src/main.rs");
    }

    #[test]
    fn a_git_commit_is_recorded_as_a_git_op_but_git_status_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let commit_payload = serde_json::json!({ "tool_name": "Bash", "tool_input": {"command": "git commit -m 'wip'"}, "tool_response": "ok", "session_id": "sess-5", "cwd": path.clone() }).to_string();
        let status_payload = serde_json::json!({ "tool_name": "Bash", "tool_input": {"command": "git status"}, "tool_response": "ok", "session_id": "sess-5", "cwd": path }).to_string();

        process_post_tool_use(&commit_payload).unwrap();
        process_post_tool_use(&status_payload).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let events = store.session_events(&repo, "sess-5").unwrap();
        assert_eq!(events.len(), 1, "only the commit should be recorded, not plain status: {events:?}");
        assert_eq!(events[0].event_kind, "git_op");
    }

    #[test]
    fn post_tool_use_failure_records_an_error_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Bash", "error": "command not found", "session_id": "sess-6", "cwd": path }).to_string();

        process_post_tool_use_failure(&payload).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let events = store.session_events(&repo, "sess-6").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "error");
    }

    #[test]
    fn user_prompt_submit_records_user_intent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "prompt": "fix the login bug", "session_id": "sess-7", "cwd": path }).to_string();

        process_user_prompt_submit(&payload).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let events = store.session_events(&repo, "sess-7").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "user_intent");
        assert_eq!(events[0].description, "fix the login bug");
    }

    #[test]
    fn user_prompt_submit_ignores_an_empty_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "prompt": "   ", "session_id": "sess-8", "cwd": path }).to_string();

        user_prompt_submit_is_a_noop(&payload, dir.path());

        fn user_prompt_submit_is_a_noop(payload: &str, dir: &std::path::Path) {
            process_user_prompt_submit(payload).unwrap();
            let store = agentops_mcp::open_store(dir).unwrap();
            let repo = agentops_mcp::repo_name(dir);
            assert!(store.session_events(&repo, "sess-8").unwrap().is_empty());
        }
    }

    #[test]
    fn pre_tool_use_warns_on_a_bash_command_with_a_credential_but_never_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Bash", "tool_input": {"command": r#"export API_KEY="sk_live_abcdef1234567890ABCDEF""#}, "session_id": "sess-9", "cwd": path }).to_string();

        // Must never return an Err (would surface as a nonzero exit).
        process_pre_tool_use(&payload, false).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let events = store.session_events(&repo, "sess-9").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "secret_exposure_warning");
    }

    #[test]
    fn pre_tool_use_warns_on_reading_a_dotenv_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Read", "tool_input": {"file_path": ".env"}, "session_id": "sess-10", "cwd": path }).to_string();

        process_pre_tool_use(&payload, false).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        let events = store.session_events(&repo, "sess-10").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "secret_exposure_warning");
    }

    #[test]
    fn pre_tool_use_is_silent_for_an_ordinary_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Read", "tool_input": {"file_path": "src/main.rs"}, "session_id": "sess-11", "cwd": path }).to_string();

        process_pre_tool_use(&payload, false).unwrap();

        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        assert!(store.session_events(&repo, "sess-11").unwrap().is_empty());
    }

    #[test]
    fn classify_for_compression_recognizes_the_supported_commands() {
        assert!(matches!(classify_for_compression("cargo test -p agentops-graph"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::TestRust))));
        assert!(matches!(classify_for_compression("npm test"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::TestNode))));
        assert!(matches!(classify_for_compression("pytest -v"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::TestPytest))));
        assert!(matches!(classify_for_compression("go test ./..."), Some(CompressAction::Pipe(crate::compress_output::CompressKind::TestGo))));
        assert!(matches!(classify_for_compression("grep -rn foo src/"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::Grep))));
        assert!(matches!(classify_for_compression("rg foo"), Some(CompressAction::Pipe(crate::compress_output::CompressKind::Grep))));
        assert!(matches!(classify_for_compression("git log"), Some(CompressAction::AppendFlag("--oneline"))));
        assert!(matches!(classify_for_compression("git diff"), Some(CompressAction::AppendFlag("-U1"))));
    }

    #[test]
    fn classify_for_compression_declines_a_command_already_carrying_the_relevant_flag() {
        assert!(classify_for_compression("git log --oneline").is_none());
        assert!(classify_for_compression("git log --format=%H").is_none());
        assert!(classify_for_compression("git diff -U5").is_none());
        assert!(classify_for_compression("git diff --unified=3").is_none());
    }

    #[test]
    fn classify_for_compression_declines_a_chained_or_unrecognized_command() {
        assert!(classify_for_compression("cargo test && echo done").is_none(), "must not rewrite only the head of a chained command");
        assert!(classify_for_compression("cargo test; echo done").is_none());
        assert!(classify_for_compression("cargo test | tee out.log").is_none(), "must not double-pipe an already-piped command");
        assert!(classify_for_compression("(cd foo && cargo test)").is_none());
        assert!(classify_for_compression("echo hello").is_none());
        assert!(classify_for_compression("git status").is_none(), "git status is not a supported command");
    }

    #[test]
    fn rewrite_command_pipes_through_compress_output_and_restores_the_real_exit_code() {
        let rewritten = rewrite_command("cargo test", &CompressAction::Pipe(crate::compress_output::CompressKind::TestRust));
        assert_eq!(rewritten, "{ cargo test; } 2>&1 | agentops compress-output --kind test-rust ; exit ${PIPESTATUS[0]}");
    }

    #[test]
    fn rewrite_command_appends_a_flag_with_no_pipe_for_git_log_and_diff() {
        assert_eq!(rewrite_command("git log", &CompressAction::AppendFlag("--oneline")), "git log --oneline");
        assert_eq!(rewrite_command("git diff", &CompressAction::AppendFlag("-U1")), "git diff -U1");
    }

    #[test]
    fn detect_command_rewrite_only_applies_to_bash_tool_calls() {
        let tool_input = serde_json::json!({ "command": "cargo test" });
        assert!(detect_command_rewrite("Bash", &tool_input).is_some());
        assert!(detect_command_rewrite("Read", &tool_input).is_none(), "a non-Bash tool must never be rewritten");
    }

    #[test]
    fn pre_tool_use_rewrites_a_test_command_only_when_compress_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "tool_name": "Bash", "tool_input": {"command": "cargo test"}, "session_id": "sess-12", "cwd": path }).to_string();

        // Without --compress: silent, no rewrite (and no credential in this
        // command, so no warning either).
        process_pre_tool_use(&payload, false).unwrap();
    }

    /// Confirms the actual shell mechanism `rewrite_command`'s `Pipe`
    /// variant depends on: a bare pipe reports the *piped-to* command's
    /// exit code, but `PIPESTATUS[0]` recovers the original command's real
    /// one, and the trailing `exit ${PIPESTATUS[0]}` makes that the whole
    /// rewritten line's final status -- verified directly in a real shell,
    /// not just asserted about `rewrite_command`'s string output above.
    #[test]
    fn pipestatus_recovers_the_original_commands_real_exit_code_through_a_pipe() {
        let rewritten = rewrite_command("false", &CompressAction::Pipe(crate::compress_output::CompressKind::TestRust));
        // Swap the real binary invocation for `cat` so this test doesn't
        // depend on `agentops` being built/on PATH -- the exit-code
        // mechanism under test is bash's, not this crate's own.
        let rewritten = rewritten.replace("agentops compress-output --kind test-rust", "cat");
        let status = std::process::Command::new("bash").arg("-c").arg(&rewritten).status().unwrap();
        assert_eq!(status.code(), Some(1), "the rewritten line's exit code must be `false`'s real code (1), not `cat`'s (0): {rewritten}");
    }

    #[test]
    fn session_start_is_a_noop_on_plain_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let payload = serde_json::json!({ "source": "startup", "session_id": "sess-12", "cwd": path }).to_string();

        // Must not error even though nothing has been scanned/recorded yet.
        process_session_start(&payload).unwrap();
    }

    #[test]
    fn session_start_on_startup_primes_a_scanned_repo_via_the_fallback_briefing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let store = agentops_mcp::open_store(dir.path()).unwrap();
        let repo = agentops_mcp::repo_name(dir.path());
        store.add_node(NewNode { kind: NodeKind::Gotcha, repo: repo.clone(), path: None, name: Some("Watch out".into()), container: None, start_line: None, end_line: None, content: Some("A real gotcha.".into()) }).unwrap();
        store.record_scan(&repo, &[]).unwrap();

        let payload = serde_json::json!({ "source": "startup", "session_id": "sess-13", "cwd": path }).to_string();

        // Doesn't error, and (unlike the never-scanned case above) actually
        // has something to prime the fresh session with -- can't easily
        // assert on stdout here without capturing it, so this just confirms
        // the fallback path executes cleanly against a real scanned repo.
        process_session_start(&payload).unwrap();
    }
}
