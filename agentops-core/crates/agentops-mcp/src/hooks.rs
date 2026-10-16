//! The store-touching half of every Claude Code hook `agentops hook-capture`
//! handles. One implementation, two callers: `agentops-cli`'s hook handler
//! calls [`handle`] directly against the local store, and a repo connected
//! with `agentops connect --remote` forwards the same payload to the
//! server's `POST /repos/{id}/hooks/{event}`, which calls [`handle`] against
//! the server's store. The two paths can't drift because they run this
//! same function.
//!
//! Everything that doesn't need a store stays in `agentops-cli`
//! (`--compress` command rewriting, reading stdin, the detached
//! `sessionend` usage sync). [`is_relevant`] is also called client-side,
//! so a remote repo doesn't make an HTTP call for every ordinary tool call.
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
//! - `pretooluse`: a proactive secret-exposure warning. It never blocks.
//! - `sessionstart`: renders the session-resume guide on compact/resume/
//!   startup, via `session_guide::build`.
//!
//! [`handle`] returns the JSON object the hook prints on stdout, or `None`
//! when the hook has nothing to say.

use std::path::Path;

use agentops_graph::{GraphStore, NewNode, NodeKind};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::budget::{cap, DEFAULT_CHAR_BUDGET};

/// The `--event` names whose handling needs a store. Anything else is
/// client-only or unknown.
pub const STORE_EVENTS: &[&str] = &["posttooluse", "posttoolusefailure", "userpromptsubmit", "sessionstart", "pretooluse", "subagentstart"];

/// Subagent types `subagentstart` seeds. Planning and review agents: the
/// ones that read code before anything is decided. Matched exactly on the
/// payload's `agent_type`.
pub const SEEDED_AGENTS: &[&str] = &["Explore", "Plan", "plan-council-member", "plan-council-chair"];

/// Runs `event`'s store-touching logic for a raw hook `payload` against
/// `repo` in `store`. The payload's `cwd` is never used to pick the repo:
/// the caller already resolved it (from `cwd` locally, from the connection
/// on the server).
pub fn handle(store: &dyn GraphStore, repo: &str, event: &str, payload: &Value) -> Result<Option<Value>> {
    match event {
        "posttooluse" => post_tool_use(store, repo, payload).map(|()| None),
        "posttoolusefailure" => post_tool_use_failure(store, repo, payload).map(|()| None),
        "userpromptsubmit" => user_prompt_submit(store, repo, payload),
        "sessionstart" => session_start(store, repo, payload),
        "pretooluse" => pre_tool_use(store, repo, payload),
        "subagentstart" => subagent_start(store, repo, payload),
        other => anyhow::bail!("unknown hook event '{other}'"),
    }
}

/// Whether `event` with this `payload` could do anything at all. Cheap and
/// store-free, so the CLI can skip a remote round trip (or a local store
/// open) for the common case: a `Read` that isn't an edit, a git op, a web
/// reference, or a large output, and a `PreToolUse` with nothing to warn
/// about.
pub fn is_relevant(event: &str, payload: &Value) -> bool {
    let tool_name = payload.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let tool_input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    match event {
        "posttooluse" => recorded_post_tool_event(payload, tool_name, &tool_input).is_some() || render_tool_response(payload.get("tool_response").unwrap_or(&Value::Null)).chars().count() > DEFAULT_CHAR_BUDGET,
        "pretooluse" => tool_name == "ExitPlanMode" || is_memory_write(tool_name, &tool_input) || detect_secret_exposure(tool_name, &tool_input).is_some(),
        "userpromptsubmit" => payload.get("prompt").and_then(|v| v.as_str()).is_some_and(|p| !p.trim().is_empty() && !is_task_notification(p)),
        "subagentstart" => payload.get("agent_type").and_then(|v| v.as_str()).is_some_and(|t| SEEDED_AGENTS.contains(&t)),
        _ => true,
    }
}

/// The JSON every context-injecting hook prints. `additionalContext` goes
/// inside `hookSpecificOutput`; `systemMessage` (a warning shown to the
/// user) is one of Claude Code's top-level output fields, a sibling of
/// `hookSpecificOutput`. Each is omitted when `None`.
pub fn hook_output(event_name: &str, additional_context: Option<&str>, system_message: Option<&str>) -> Value {
    let mut specific = serde_json::json!({ "hookEventName": event_name });
    if let Some(context) = additional_context {
        specific["additionalContext"] = Value::String(context.to_string());
    }
    let mut output = serde_json::json!({ "hookSpecificOutput": specific });
    if let Some(message) = system_message {
        output["systemMessage"] = Value::String(message.to_string());
    }
    output
}

fn session_id(payload: &Value) -> &str {
    payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("unknown-session")
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
    tool_input: Value,
    #[serde(default)]
    tool_response: Value,
}

fn post_tool_use(store: &dyn GraphStore, repo: &str, payload: &Value) -> Result<()> {
    let session_id = session_id(payload);
    let raw = payload;
    let payload: PostToolUsePayload = serde_json::from_value(payload.clone()).context("parsing PostToolUse payload")?;

    // Cheap, always-on categorization -- independent of whether this call's
    // own output is large enough to warrant the full-capture path below.
    // Most calls match nothing here, and that's fine: unlike the MCP tools'
    // own event recording, there's no catch-all "activity" fallback: not
    // every tool call is guide-worthy.
    if let Some((event_kind, description)) = recorded_post_tool_event(raw, &payload.tool_name, &payload.tool_input) {
        store.record_session_event(repo, session_id, &payload.tool_name, &description, None, event_kind)?;
    }

    let text = render_tool_response(&payload.tool_response);
    if text.chars().count() <= DEFAULT_CHAR_BUDGET {
        // Small enough to not be worth capturing — the whole point is only
        // ever intervening on the outputs that would otherwise bloat context.
        return Ok(());
    }

    // Stored in full (redacted, not truncated) — the whole point of a
    // Node-backed capture is that `fetch_content`/lexical search can reach
    // *all* of it later; only the inline tool responses in this crate
    // (get_symbol, related_context, explain_symbol) cap what they return.
    let redaction = agentops_security::redact(&text);

    let node_id = store.add_node(NewNode {
        kind: NodeKind::ToolOutput,
        repo: repo.to_string(),
        path: Some(format!("tool_output:{session_id}:{}", now_nanos())),
        name: Some(payload.tool_name.clone()),
        container: None,
        start_line: None,
        end_line: None,
        content: Some(redaction.text),
    })?;

    store.record_session_event(repo, session_id, &payload.tool_name, &format!("captured {} chars of tool output ({} secret(s) redacted)", text.chars().count(), redaction.redacted_count), Some(node_id), "tool_output")?;

    // CacheAligner-equivalent (Headroom-inspired, see the project's own
    // notes on this): this captured `ToolOutput` node is exactly the kind
    // of content a later session might reuse/re-embed verbatim -- flag it
    // if it carries volatile content (a fresh timestamp, session UUID/JWT,
    // or hash) that would look stale or misleading by the time it's reused.
    // Detector-only, same as `detect_cache_unsafe` itself: never mutates
    // the captured content, only warns. Best-effort, same as the warning
    // above it -- a failure to record this must never fail the hook.
    let hazards = crate::budget::detect_cache_unsafe(&text);
    if !hazards.is_empty() {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for hazard in &hazards {
            let kind = match hazard.kind {
                crate::budget::CacheHazardKind::Uuid => "UUID",
                crate::budget::CacheHazardKind::Jwt => "JWT",
                crate::budget::CacheHazardKind::HexHash => "hex hash",
                crate::budget::CacheHazardKind::Timestamp => "timestamp",
            };
            *counts.entry(kind).or_default() += 1;
        }
        let summary = counts.into_iter().map(|(kind, n)| format!("{n} {kind}")).collect::<Vec<_>>().join(", ");
        let _ = store.record_session_event(repo, session_id, &payload.tool_name, &format!("captured tool output contains volatile content ({summary}) — may look stale if reused verbatim in a later session"), Some(node_id), "cache_hazard_warning");
    }

    Ok(())
}

/// Feeds `get_session_guide`'s "Files Modified"/"Git Ops"/"External
/// References" sections. Deliberately narrow, pattern-based, no LLM call --
/// "Rejected Approaches"/"Constraints"/"Blockers" need actual semantic
/// judgment and are out of scope here (see the plan this was built from).
fn classify_post_tool_use(tool_name: &str, tool_input: &Value) -> Option<(&'static str, String)> {
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
        "Skill" => {
            let skill = tool_input.get("skill").and_then(|v| v.as_str())?;
            FLOW_SKILLS.contains(&skill).then(|| (FLOW_SKILL, skill.to_string()))
        }
        name if RECALL_TOOLS.contains(&name) => Some((RECALL_CALL, name.to_string())),
        _ => None,
    }
}

/// `classify_post_tool_use`, minus task-start-flow evidence from a
/// subagent: a council member's recall must not stand in for the
/// planner's. Subagent payloads carry `agent_id` (logged live on Claude
/// Code 2.1.286, 2026-10-16); the main thread's don't.
fn recorded_post_tool_event(payload: &Value, tool_name: &str, tool_input: &Value) -> Option<(&'static str, String)> {
    let event = classify_post_tool_use(tool_name, tool_input)?;
    let from_subagent = payload.get("agent_id").is_some();
    (!(from_subagent && matches!(event.0, RECALL_CALL | FLOW_SKILL))).then_some(event)
}

/// `event_kind`s recording that the task-start flow actually ran, read back
/// by `check_presented_plan`.
pub const RECALL_CALL: &str = "recall_call";
pub const FLOW_SKILL: &str = "flow_skill";

/// The agentops tools that count as recall, and the skills the flow
/// requires in every planning episode.
const RECALL_TOOLS: &[&str] = &["mcp__agentops__related_context", "mcp__agentops__get_symbol", "mcp__agentops__list_gotchas", "mcp__agentops__fetch_content", "mcp__agentops__get_session_guide"];
const FLOW_SKILLS: &[&str] = &["plan", "library-docs", "audit-plan"];

/// A plain substring check, not a regex — no new dependency needed for
/// something this simple.
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
fn render_tool_response(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Object(map) => match map.get("text").or_else(|| map.get("content")).and_then(|v| v.as_str()) {
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
/// `error` is a best-effort guess. Defaults to `Value::Null` if the real
/// field is named something else, which `render_tool_response` degrades to
/// an empty description for -- a silently-thin event, never a parse failure.
#[derive(Debug, Deserialize)]
struct PostToolUseFailurePayload {
    tool_name: String,
    #[serde(default)]
    error: Value,
}

fn post_tool_use_failure(store: &dyn GraphStore, repo: &str, payload: &Value) -> Result<()> {
    let session_id = session_id(payload);
    let payload: PostToolUseFailurePayload = serde_json::from_value(payload.clone()).context("parsing PostToolUseFailure payload")?;
    let description = render_tool_response(&payload.error);
    let capped = cap(&description, DEFAULT_CHAR_BUDGET, 0);
    let redacted = agentops_security::redact(&capped.text);
    store.record_session_event(repo, session_id, &payload.tool_name, &redacted.text, None, "error")?;
    Ok(())
}

// ---------------------------------------------------------------------
// UserPromptSubmit
// ---------------------------------------------------------------------

/// `tool_name` of the `user_intent` event a prompt is recorded as. The
/// plan-mode variant is how the next prompt tells whether a planning
/// episode is already under way; always compare it exactly.
pub const PROMPT_TOOL: &str = "user_prompt";
pub const PLAN_PROMPT_TOOL: &str = "user_prompt:plan";
/// `event_kind` recording which node ids a hook already showed this
/// session, so recall never repeats itself (description: `ids 1,2,3`).
pub const RECALL_INJECTED: &str = "recall_injected";

/// The task-start flow directive (`prompts/shared/plan-flow.md`), compiled in.
fn plan_flow_directive(agentops_path: &str) -> String {
    include_str!("../../../prompts/shared/plan-flow.md").replace("{MAX_PLAN_QUESTIONS}", &crate::budget::MAX_PLAN_QUESTIONS.to_string()).replace("{AGENTOPS_PATH}", agentops_path)
}

/// The `path` agents should pass to agentops tools: what `agentops
/// hook-capture` put in the payload (the connection id for a remote repo),
/// else this store's repo name.
fn agentops_path<'a>(payload: &'a Value, repo: &'a str) -> &'a str {
    payload.get("agentops_path").and_then(|v| v.as_str()).filter(|p| !p.is_empty()).unwrap_or(repo)
}

/// Sent on every later prompt of a planning episode instead of the full
/// directive: never restart the flow mid-episode.
const PLAN_FLOW_REMINDER: &str = "AgentOps task-start flow: you're mid-flow. Carry on from the step you're on (recall, library-docs, pointed questions, audit-plan, then present with \"Research performed\" and \"Council verdict\"). Don't restart it.";

/// Claude Code delivers a finished background task as a `UserPromptSubmit`
/// whose prompt is the notification XML (spike, 2026-10-14), and a
/// subagent's hand-back as one whose prompt is `<agent-message from=...>`
/// (recorded hook payload, 2026-10-16; the transcript shows an extra
/// "Another Claude session sent a message:" line the hook never sees).
/// Neither is the user speaking, so neither is recorded or used for recall.
fn is_task_notification(prompt: &str) -> bool {
    let prompt = prompt.trim_start();
    prompt.starts_with("<task-notification>") || prompt.starts_with("<agent-message")
}

/// The session's most recent real user prompt, from either mode. Feeds the
/// subagent seed, which has no prompt of its own.
pub fn last_prompt(store: &dyn GraphStore, repo: &str, session_id: &str) -> Result<Option<String>> {
    Ok(store.session_events(repo, session_id)?.into_iter().rev().find(|e| e.event_kind == "user_intent" && (e.tool_name == PROMPT_TOOL || e.tool_name == PLAN_PROMPT_TOOL)).map(|e| e.description))
}

/// Node ids a hook already injected in this session.
fn injected_ids(events: &[agentops_graph::SessionEvent]) -> std::collections::HashSet<i64> {
    events.iter().filter(|e| e.event_kind == RECALL_INJECTED).flat_map(|e| e.description.trim_start_matches("ids ").split(',').filter_map(|id| id.trim().parse().ok()).collect::<Vec<i64>>()).collect()
}

/// Appends a recall section to `parts` with as many pointers as fit in
/// `budget` (counting everything already in `parts`), and records which
/// ones went out. Returns nothing; an empty seed adds nothing.
fn push_seed(store: &dyn GraphStore, repo: &str, session_id: &str, parts: &mut Vec<String>, seed: &[agentops_retrieval::RecallPointer], budget: usize) -> Result<()> {
    const HEADER: &str = "Recalled from AgentOps for this task (pointers; `fetch_content` with the id returns the full text):";
    let used: usize = parts.iter().map(|p| p.chars().count() + 2).sum::<usize>() + HEADER.chars().count();
    let mut lines = Vec::new();
    let mut size = used;
    for pointer in seed {
        let line = pointer.render();
        size += line.chars().count() + 1;
        if size > budget {
            break;
        }
        lines.push((pointer.node_id, line));
    }
    if lines.is_empty() {
        return Ok(());
    }
    let ids = lines.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>().join(",");
    store.record_session_event(repo, session_id, "recall", &format!("ids {ids}"), None, RECALL_INJECTED)?;
    parts.push(format!("{HEADER}\n{}", lines.into_iter().map(|(_, l)| l).collect::<Vec<_>>().join("\n")));
    Ok(())
}

/// Records the prompt, then injects context:
/// - always: pinned notes (sticky pins re-injected on every turn, the
///   "combat context drift" mechanism -- a pin is a standing directive);
/// - plan mode, first prompt of a planning episode (the previous prompt
///   wasn't in plan mode, or there was none): the task-start flow directive
///   and a recall seed of up to `PLAN_SEED_MAX_ITEMS` pointers;
/// - plan mode, later prompts (answers, refinements): a one-line reminder;
/// - any other mode: up to `DEFAULT_SEED_MAX_ITEMS` pointers not already
///   shown this session, so a new topic gets fresh recall and a follow-up
///   on the same topic gets nothing extra.
///
/// Everything fits in `PLAN_FLOW_BUDGET` plus `PIN_BUDGET` for the pins;
/// recall pointers are dropped first.
fn user_prompt_submit(store: &dyn GraphStore, repo: &str, payload: &Value) -> Result<Option<Value>> {
    use crate::budget::{DEFAULT_SEED_MAX_ITEMS, PLAN_FLOW_BUDGET, PLAN_SEED_MAX_ITEMS};

    let Some(prompt) = payload.get("prompt").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    if prompt.trim().is_empty() || is_task_notification(prompt) {
        return Ok(None);
    }
    let session_id = session_id(payload);
    let plan_mode = payload.get("permission_mode").and_then(|v| v.as_str()) == Some("plan");
    let events = store.session_events(repo, session_id)?;
    let continuing_episode = events.iter().rev().find(|e| e.event_kind == "user_intent").is_some_and(|e| e.tool_name == PLAN_PROMPT_TOOL);

    let capped = cap(prompt, DEFAULT_CHAR_BUDGET, 0);
    let redacted = agentops_security::redact(&capped.text);
    store.record_session_event(repo, session_id, if plan_mode { PLAN_PROMPT_TOOL } else { PROMPT_TOOL }, &redacted.text, None, "user_intent")?;

    let mut parts = Vec::new();
    let pinned = crate::session_guide::build_pinned_context(store, repo)?;
    if !pinned.is_empty() {
        parts.push(pinned);
    }
    if plan_mode && continuing_episode {
        parts.push(PLAN_FLOW_REMINDER.to_string());
    } else if plan_mode {
        parts.push(plan_flow_directive(agentops_path(payload, repo)));
        // The whole seed, even pointers shown earlier in the session: the
        // plan is built from this, and earlier context may be compacted.
        let seed = agentops_retrieval::recall_seed(store, repo, prompt, PLAN_SEED_MAX_ITEMS, &Default::default())?;
        push_seed(store, repo, session_id, &mut parts, &seed, PLAN_FLOW_BUDGET + crate::budget::PIN_BUDGET)?;
    } else {
        let seed = agentops_retrieval::recall_seed(store, repo, prompt, DEFAULT_SEED_MAX_ITEMS, &injected_ids(&events))?;
        push_seed(store, repo, session_id, &mut parts, &seed, PLAN_FLOW_BUDGET + crate::budget::PIN_BUDGET)?;
    }
    if parts.is_empty() {
        return Ok(None);
    }
    let context = cap(&parts.join("\n\n"), PLAN_FLOW_BUDGET + crate::budget::PIN_BUDGET, 0).text;
    Ok(Some(hook_output("UserPromptSubmit", Some(&context), None)))
}

// ---------------------------------------------------------------------
// PreToolUse
// ---------------------------------------------------------------------

/// A proactive secret-exposure warning that reuses the existing
/// redaction/filename-classification code directly. **Never sets
/// `permissionDecision`**: the tool call always proceeds through Claude
/// Code's normal permission flow untouched. Recording the warning is
/// best-effort; the warning itself is the point.
fn pre_tool_use(store: &dyn GraphStore, repo: &str, payload: &Value) -> Result<Option<Value>> {
    let tool_name = payload.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let tool_input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
    if tool_name == "ExitPlanMode" {
        return check_presented_plan(store, repo, session_id(payload), &tool_input);
    }
    if is_memory_write(tool_name, &tool_input) {
        let path = tool_input.get("file_path").and_then(|v| v.as_str()).unwrap_or_default();
        let _ = store.record_session_event(repo, session_id(payload), tool_name, path, None, MEMORY_WRITE_REDIRECTED);
        return Ok(Some(serde_json::json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": MEMORY_REDIRECT_REASON } })));
    }
    let Some(warning) = detect_secret_exposure(tool_name, &tool_input) else {
        return Ok(None);
    };
    let _ = store.record_session_event(repo, session_id(payload), tool_name, &warning, None, "secret_exposure_warning");
    Ok(Some(hook_output("PreToolUse", Some(&warning), Some(&format!("agentops: {warning}")))))
}

/// `event_kind` of a write to Claude Code's local memory that was sent to
/// `add_note` instead.
pub const MEMORY_WRITE_REDIRECTED: &str = "memory_write_redirected";

const MEMORY_REDIRECT_REASON: &str = "AgentOps is this repo's memory: Claude Code's local memory files (MEMORY.md and the files beside it) aren't written here. Record this with the add_note MCP tool instead (it reaches the remote server when the repo is connected to one). Set note_type to gotcha, decision or knowledge, and add one tag saying what kind of memory it is: memory-kind:semantic (a fact and the reason behind it), memory-kind:episodic (what happened in a session), or memory-kind:procedural (a rule that must always hold). If it updates something already recorded, say so in the body.";

/// A `Write`/`Edit`/`MultiEdit` into Claude Code's per-project memory
/// directory (`~/.claude/projects/<project>/memory/`). Reads aren't
/// touched: existing MEMORY.md files stay readable until they're imported
/// (milestone 2). Backslashes are normalized so a Windows path matches too.
fn is_memory_write(tool_name: &str, tool_input: &Value) -> bool {
    if !matches!(tool_name, "Write" | "Edit" | "MultiEdit") {
        return false;
    }
    let Some(path) = tool_input.get("file_path").and_then(|v| v.as_str()) else { return false };
    let path = path.replace('\\', "/");
    path.split_once("/.claude/projects/").is_some_and(|(_, rest)| rest.split('/').nth(1) == Some("memory"))
}

/// `event_kind` of a plan sent back for a missing section, and of one let
/// through anyway with a warning.
pub const PLAN_NUDGE: &str = "plan_nudge";
pub const PLAN_PRESENTED_INCOMPLETE: &str = "plan_presented_incomplete";

/// Sections every presented plan must carry (headings, matched
/// case-insensitively), with what to do when one is missing.
const REQUIRED_PLAN_SECTIONS: &[(&str, &str)] = &[
    ("Research performed", "recall first (the plan skill's gotcha/decision check) and record it under a \"Research performed\" heading"),
    ("Council verdict", "run the audit-plan skill and put its verdict under a \"Council verdict\" heading"),
];

/// Soft enforcement of the task-start flow (milestone 1, item 6). On
/// Claude Code 2.1.286 a `PreToolUse` deny on `ExitPlanMode` is obeyed: the
/// plan isn't shown and the agent gets the reason (spike, 2026-10-14). The
/// first plan in a planning episode that lacks a required section, or
/// whose episode has no recorded recall call or flow skill run (see
/// `recorded_post_tool_event`), is denied with a reason naming what's
/// missing. After
/// `ENFORCE_MAX_NUDGES` nudges it goes through with a warning shown to the
/// user, so this can never loop. An episode starts after the last
/// ordinary-mode prompt. If `tool_input.plan` is missing (another Claude
/// Code version), the plan goes through untouched.
fn check_presented_plan(store: &dyn GraphStore, repo: &str, session_id: &str, tool_input: &Value) -> Result<Option<Value>> {
    let Some(plan) = tool_input.get("plan").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let headings: Vec<String> = plan.lines().filter(|l| l.trim_start().starts_with('#')).map(|l| l.to_lowercase()).collect();
    let missing: Vec<&(&str, &str)> = REQUIRED_PLAN_SECTIONS.iter().filter(|(name, _)| !headings.iter().any(|h| h.contains(&name.to_lowercase()))).collect();

    // Evidence that the flow ran, from this episode only (after the last
    // ordinary-mode prompt). Headings alone let a plan that skipped recall
    // through (milestone 1 check 1, 2026-10-16).
    let events = store.session_events(repo, session_id)?;
    let episode = events.iter().rposition(|e| e.event_kind == "user_intent" && e.tool_name == PROMPT_TOOL).map_or(&events[..], |i| &events[i + 1..]);
    let mut unrun = Vec::new();
    if !episode.iter().any(|e| e.event_kind == RECALL_CALL) {
        unrun.push("recall (related_context, get_symbol or list_gotchas)".to_string());
    }
    for skill in FLOW_SKILLS {
        if !episode.iter().any(|e| e.event_kind == FLOW_SKILL && e.description == *skill) {
            unrun.push(format!("the {skill} skill"));
        }
    }
    if missing.is_empty() && unrun.is_empty() {
        return Ok(None);
    }

    let mut problems = Vec::new();
    if !missing.is_empty() {
        problems.push(format!("this plan has no {} section", missing.iter().map(|(name, _)| format!("\"{name}\"")).collect::<Vec<_>>().join(" and ")));
    }
    if !unrun.is_empty() {
        problems.push(format!("there's no sign you ran {}", unrun.join(", ")));
    }
    let problems = problems.join("; ");
    let nudges_this_episode = episode.iter().filter(|e| e.event_kind == PLAN_NUDGE).count();
    if nudges_this_episode < crate::budget::ENFORCE_MAX_NUDGES {
        store.record_session_event(repo, session_id, "ExitPlanMode", &format!("plan sent back: {problems}"), None, PLAN_NUDGE)?;
        let mut steps: Vec<String> = unrun.iter().map(|step| format!("run {step}")).collect();
        steps.extend(missing.iter().map(|(_, step)| step.to_string()));
        let reason = format!("AgentOps task-start flow: {problems}. Before presenting it, {}. Then call ExitPlanMode again.", steps.join("; then "));
        return Ok(Some(serde_json::json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason } })));
    }
    store.record_session_event(repo, session_id, "ExitPlanMode", &format!("plan presented anyway: {problems}"), None, PLAN_PRESENTED_INCOMPLETE)?;
    Ok(Some(hook_output("PreToolUse", None, Some(&format!("agentops: this plan skipped part of the task-start flow ({problems}). Review it with that in mind.")))))
}

fn detect_secret_exposure(tool_name: &str, tool_input: &Value) -> Option<String> {
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
// SubagentStart
// ---------------------------------------------------------------------

/// Subagents load no CLAUDE.md or AGENTS.md (spike, 2026-10-14), so this
/// hook is the only way project memory reaches them. For a planning or
/// review agent (`SEEDED_AGENTS`), injects the pinned notes and a recall
/// seed built from the session's latest user prompt (the payload carries no
/// task of its own), within `SUBAGENT_SEED_BUDGET`. Nothing is recorded as
/// injected: a subagent's context isn't the main session's, so the main
/// session must still be able to see these pointers itself.
fn subagent_start(store: &dyn GraphStore, repo: &str, payload: &Value) -> Result<Option<Value>> {
    use crate::budget::{PLAN_SEED_MAX_ITEMS, SUBAGENT_SEED_BUDGET};

    if !is_relevant("subagentstart", payload) {
        return Ok(None);
    }
    // Every seeded agent learns which `path` to pass. The council's own
    // prompt already says to look things up before grepping and haiku
    // ignored it (milestone 1 check 3), so it's repeated here, first, as
    // an experiment the next check 3 run decides.
    let path = agentops_path(payload, repo);
    let mut parts = vec![if payload.get("agent_type").and_then(|v| v.as_str()).is_some_and(|t| t.starts_with("plan-council-")) {
        format!("First, before any Read, Grep or Bash: call `get_symbol`, `related_context` or `list_gotchas` (`path: \"{path}\"`) for each file and symbol the plan names.")
    } else {
        format!("Pass `path: \"{path}\"` to every agentops tool.")
    }];
    let pinned = crate::session_guide::build_pinned_context(store, repo)?;
    if !pinned.is_empty() {
        parts.push(pinned);
    }
    if let Some(prompt) = last_prompt(store, repo, session_id(payload))? {
        let seed = agentops_retrieval::recall_seed(store, repo, &prompt, PLAN_SEED_MAX_ITEMS, &Default::default())?;
        if !seed.is_empty() {
            const HEADER: &str = "Recalled from AgentOps for the task you were spawned for (pointers; query `get_symbol`/`related_context`/`list_gotchas` or `fetch_content` with the id before grepping):";
            parts.push(format!("{HEADER}\n{}", seed.iter().map(|p| p.render()).collect::<Vec<_>>().join("\n")));
        }
    }
    let context = cap(&parts.join("\n\n"), SUBAGENT_SEED_BUDGET + crate::budget::PIN_BUDGET, 0).text;
    Ok(Some(hook_output("SubagentStart", Some(&context), None)))
}

// ---------------------------------------------------------------------
// SessionStart
// ---------------------------------------------------------------------

fn session_start(store: &dyn GraphStore, repo: &str, payload: &Value) -> Result<Option<Value>> {
    // `source` is one of startup/resume/clear/compact/fork. AgentOps never
    // needs a `PreCompact` hook because `session_events` already persists
    // durably and incrementally throughout the session -- gating here is
    // the only "checkpoint" logic needed (ADR 0035). `startup` is included
    // so a brand-new session also gets primed: there's nothing yet to
    // resume, but `build_repo_briefing`'s fallback still has real
    // repo-level context to offer.
    let source = payload.get("source").and_then(|v| v.as_str());
    if !matches!(source, Some("compact") | Some("resume") | Some("startup")) {
        return Ok(None);
    }
    let Some(session_id) = payload.get("session_id").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let guide = crate::session_guide::build(store, repo, session_id)?;
    let guide = if guide.is_empty() { crate::session_guide::build_repo_briefing(store, repo)? } else { guide };
    // Sticky pins are a standing directive, prepended ahead of the
    // session-resume guide/briefing -- and checked for emptiness only after
    // this concatenation, so a session with pins but no other activity yet
    // still emits output.
    let pinned = crate::session_guide::build_pinned_context(store, repo)?;
    let guide = if pinned.is_empty() { guide } else if guide.is_empty() { pinned } else { format!("{pinned}\n\n{guide}") };
    Ok((!guide.is_empty()).then(|| hook_output("SessionStart", Some(&guide), None)))
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
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Box<dyn GraphStore>, String) {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::open_store(dir.path()).unwrap();
        let repo = crate::repo_name(dir.path());
        (dir, store, repo)
    }

    #[test]
    fn render_tool_response_passes_through_a_plain_string() {
        assert_eq!(render_tool_response(&json!("hello")), "hello");
    }

    #[test]
    fn render_tool_response_extracts_text_from_the_real_claude_code_shape() {
        assert_eq!(render_tool_response(&json!({"type": "text", "text": "hello world"})), "hello world");
    }

    #[test]
    fn render_tool_response_falls_back_to_stringifying_an_unrecognized_shape() {
        assert_eq!(render_tool_response(&json!({"a": 1})), r#"{"a":1}"#);
    }

    #[test]
    fn is_git_op_recognizes_common_verbs_but_not_plain_git_status() {
        assert!(is_git_op("git commit -m 'wip'"));
        assert!(is_git_op("git push origin main"));
        assert!(!is_git_op("git status"));
        assert!(!is_git_op("echo git commit"));
    }

    #[test]
    fn is_relevant_skips_an_ordinary_read_but_not_an_edit_a_large_output_or_a_secret() {
        assert!(!is_relevant("posttooluse", &json!({ "tool_name": "Read", "tool_input": {"file_path": "src/main.rs"}, "tool_response": "short" })));
        assert!(is_relevant("posttooluse", &json!({ "tool_name": "Edit", "tool_input": {"file_path": "src/main.rs"}, "tool_response": "ok" })));
        assert!(is_relevant("posttooluse", &json!({ "tool_name": "Read", "tool_response": "x".repeat(DEFAULT_CHAR_BUDGET + 1) })));
        assert!(!is_relevant("pretooluse", &json!({ "tool_name": "Read", "tool_input": {"file_path": "src/main.rs"} })));
        assert!(is_relevant("pretooluse", &json!({ "tool_name": "Read", "tool_input": {"file_path": ".env"} })));
        assert!(is_relevant("posttooluse", &json!({ "tool_name": "mcp__agentops__related_context", "tool_input": {} })));
        assert!(is_relevant("posttooluse", &json!({ "tool_name": "Skill", "tool_input": {"skill": "audit-plan"} })));
        assert!(!is_relevant("posttooluse", &json!({ "tool_name": "Skill", "tool_input": {"skill": "session"} })));
        assert!(!is_relevant("posttooluse", &json!({ "tool_name": "mcp__agentops__list_gotchas", "tool_input": {}, "agent_id": "a1" })), "a subagent's recall is never sent");
        assert!(is_relevant("userpromptsubmit", &json!({ "prompt": "hi" })));
        assert!(!is_relevant("userpromptsubmit", &json!({ "prompt": "<task-notification>x" })));
        assert!(!is_relevant("userpromptsubmit", &json!({ "prompt": "<agent-message from=\"a1\">\n[Subagent hand-back] report\n</agent-message>" })));
    }

    #[test]
    fn small_tool_output_is_not_captured() {
        let (_dir, store, repo) = store();
        handle(store.as_ref(), &repo, "posttooluse", &json!({ "tool_name": "Bash", "tool_response": "short output", "session_id": "sess-1" })).unwrap();
        assert!(store.nodes_by_kind(&repo, NodeKind::ToolOutput).unwrap().is_empty(), "small output should not be captured as a node");
    }

    #[test]
    fn large_tool_output_is_captured_redacted_and_recorded() {
        let (_dir, store, repo) = store();
        let big_output = format!("{}\nAWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLE", "line of shell output\n".repeat(300));
        handle(store.as_ref(), &repo, "posttooluse", &json!({ "tool_name": "Bash", "tool_response": big_output, "session_id": "sess-2" })).unwrap();

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
    fn large_tool_output_containing_a_uuid_also_records_a_cache_hazard_warning() {
        let (_dir, store, repo) = store();
        let big_output = format!("{}\nsession_id: 123e4567-e89b-12d3-a456-426614174000 done", "line of shell output\n".repeat(300));
        handle(store.as_ref(), &repo, "posttooluse", &json!({ "tool_name": "Bash", "tool_response": big_output, "session_id": "sess-hazard" })).unwrap();
        let events = store.session_events(&repo, "sess-hazard").unwrap();
        assert_eq!(events.len(), 2, "both the tool_output capture and the cache_hazard_warning must be recorded: {events:?}");
        assert!(events.iter().any(|e| e.event_kind == "cache_hazard_warning" && e.description.contains("UUID")), "{events:?}");
    }

    #[test]
    fn large_tool_output_with_no_volatile_content_records_no_cache_hazard_warning() {
        let (_dir, store, repo) = store();
        handle(store.as_ref(), &repo, "posttooluse", &json!({ "tool_name": "Bash", "tool_response": "line of shell output\n".repeat(300), "session_id": "sess-no-hazard" })).unwrap();
        let events = store.session_events(&repo, "sess-no-hazard").unwrap();
        assert!(!events.iter().any(|e| e.event_kind == "cache_hazard_warning"), "{events:?}");
    }

    #[test]
    fn a_real_claude_code_shaped_response_is_captured_as_plain_text_not_json_wrapped() {
        let (_dir, store, repo) = store();
        let big_text = "line of shell output\n".repeat(300);
        handle(store.as_ref(), &repo, "posttooluse", &json!({ "tool_name": "Bash", "tool_response": {"type": "text", "text": big_text}, "session_id": "sess-3" })).unwrap();
        let nodes = store.nodes_by_kind(&repo, NodeKind::ToolOutput).unwrap();
        assert_eq!(nodes.len(), 1);
        let content = nodes[0].content.as_deref().unwrap();
        assert!(!content.contains(r#""type":"text""#), "must not capture the JSON wrapper: {content}");
        assert!(content.starts_with("line of shell output"));
    }

    #[test]
    fn editing_a_file_records_a_file_modified_event_regardless_of_output_size() {
        let (_dir, store, repo) = store();
        handle(store.as_ref(), &repo, "posttooluse", &json!({ "tool_name": "Edit", "tool_input": {"file_path": "src/main.rs"}, "tool_response": "ok", "session_id": "sess-4" })).unwrap();
        let events = store.session_events(&repo, "sess-4").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "file_modified");
        assert_eq!(events[0].description, "src/main.rs");
    }

    #[test]
    fn a_git_commit_is_recorded_as_a_git_op_but_git_status_is_not() {
        let (_dir, store, repo) = store();
        handle(store.as_ref(), &repo, "posttooluse", &json!({ "tool_name": "Bash", "tool_input": {"command": "git commit -m 'wip'"}, "tool_response": "ok", "session_id": "sess-5" })).unwrap();
        handle(store.as_ref(), &repo, "posttooluse", &json!({ "tool_name": "Bash", "tool_input": {"command": "git status"}, "tool_response": "ok", "session_id": "sess-5" })).unwrap();
        let events = store.session_events(&repo, "sess-5").unwrap();
        assert_eq!(events.len(), 1, "only the commit should be recorded, not plain status: {events:?}");
        assert_eq!(events[0].event_kind, "git_op");
    }

    #[test]
    fn post_tool_use_failure_records_an_error_event() {
        let (_dir, store, repo) = store();
        handle(store.as_ref(), &repo, "posttoolusefailure", &json!({ "tool_name": "Bash", "error": "command not found", "session_id": "sess-6" })).unwrap();
        let events = store.session_events(&repo, "sess-6").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "error");
    }

    #[test]
    fn user_prompt_submit_records_user_intent() {
        let (_dir, store, repo) = store();
        handle(store.as_ref(), &repo, "userpromptsubmit", &json!({ "prompt": "fix the login bug", "session_id": "sess-7" })).unwrap();
        let events = store.session_events(&repo, "sess-7").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "user_intent");
        assert_eq!(events[0].description, "fix the login bug");
    }

    #[test]
    fn user_prompt_submit_ignores_an_empty_prompt() {
        let (_dir, store, repo) = store();
        handle(store.as_ref(), &repo, "userpromptsubmit", &json!({ "prompt": "   ", "session_id": "sess-8" })).unwrap();
        assert!(store.session_events(&repo, "sess-8").unwrap().is_empty());
    }

    #[test]
    fn pre_tool_use_warns_on_a_bash_command_with_a_credential_but_never_blocks() {
        let (_dir, store, repo) = store();
        let out = handle(store.as_ref(), &repo, "pretooluse", &json!({ "tool_name": "Bash", "tool_input": {"command": r#"export API_KEY="sk_live_abcdef1234567890ABCDEF""#}, "session_id": "sess-9" })).unwrap().unwrap();
        assert!(out["hookSpecificOutput"].get("permissionDecision").is_none(), "{out}");
        let events = store.session_events(&repo, "sess-9").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "secret_exposure_warning");
    }

    #[test]
    fn pre_tool_use_warns_on_reading_a_dotenv_file() {
        let (_dir, store, repo) = store();
        handle(store.as_ref(), &repo, "pretooluse", &json!({ "tool_name": "Read", "tool_input": {"file_path": ".env"}, "session_id": "sess-10" })).unwrap();
        let events = store.session_events(&repo, "sess-10").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "secret_exposure_warning");
    }

    #[test]
    fn pre_tool_use_is_silent_for_an_ordinary_read() {
        let (_dir, store, repo) = store();
        assert!(handle(store.as_ref(), &repo, "pretooluse", &json!({ "tool_name": "Read", "tool_input": {"file_path": "src/main.rs"}, "session_id": "sess-11" })).unwrap().is_none());
        assert!(store.session_events(&repo, "sess-11").unwrap().is_empty());
    }

    #[test]
    fn session_start_is_a_noop_on_an_unscanned_repo_and_primes_a_scanned_one() {
        let (_dir, store, repo) = store();
        // Must not error even though nothing has been scanned/recorded yet.
        handle(store.as_ref(), &repo, "sessionstart", &json!({ "source": "startup", "session_id": "sess-12" })).unwrap();

        store.add_node(NewNode { kind: NodeKind::Gotcha, repo: repo.clone(), path: None, name: Some("Watch out".into()), container: None, start_line: None, end_line: None, content: Some("A real gotcha.".into()) }).unwrap();
        store.record_scan(&repo, &[]).unwrap();
        let out = handle(store.as_ref(), &repo, "sessionstart", &json!({ "source": "startup", "session_id": "sess-13" })).unwrap().expect("a scanned repo primes a fresh session");
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
    }

    fn gotcha(store: &dyn GraphStore, repo: &str, name: &str, content: &str) -> i64 {
        agentops_graph::upsert_node(store, NewNode { kind: NodeKind::Gotcha, repo: repo.into(), path: Some(format!("vault:{name}")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap()
    }

    fn context(out: Option<Value>) -> String {
        out.map(|o| o["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_default().to_string()).unwrap_or_default()
    }

    fn prompt(store: &dyn GraphStore, repo: &str, text: &str, mode: &str) -> String {
        context(handle(store, repo, "userpromptsubmit", &json!({ "prompt": text, "session_id": "s", "permission_mode": mode })).unwrap())
    }

    #[test]
    fn the_first_plan_mode_prompt_gets_the_flow_and_a_seed_within_budget_and_later_ones_only_a_reminder() {
        let (_dir, store, repo) = store();
        let id = gotcha(store.as_ref(), &repo, "Hooks ignore the remote server", "hook capture opens the local store");
        let first = prompt(store.as_ref(), &repo, "make the hooks follow the remote server", "plan");
        assert!(first.contains("AgentOps task-start flow") && first.contains("at most 3"), "{first}");
        assert!(first.contains(&format!("(id {id})")), "the seed points at the matching gotcha: {first}");
        assert!(first.chars().count() <= crate::budget::PLAN_FLOW_BUDGET);

        let second = prompt(store.as_ref(), &repo, "yes, the server one", "plan");
        assert_eq!(second, PLAN_FLOW_REMINDER, "a later prompt in the same episode never restarts the flow");

        // Leaving plan mode and coming back starts a new episode.
        prompt(store.as_ref(), &repo, "ok go", "default");
        assert!(prompt(store.as_ref(), &repo, "plan the hooks server work again", "plan").contains("AgentOps task-start flow"));
    }

    #[test]
    fn a_default_mode_prompt_gets_only_pointers_it_hasnt_seen_this_session() {
        let (_dir, store, repo) = store();
        let hooks_id = gotcha(store.as_ref(), &repo, "Hooks ignore the remote server", "hook capture opens the local store");
        let tags_id = gotcha(store.as_ref(), &repo, "Tags dropped at ingest", "ingest keeps only the body");

        let first = prompt(store.as_ref(), &repo, "why do hooks ignore the server", "default");
        assert!(first.contains(&format!("(id {hooks_id})")) && !first.contains("task-start flow"), "{first}");
        assert!(prompt(store.as_ref(), &repo, "why do hooks ignore the server", "default").is_empty(), "the same topic again adds nothing new");
        let other = prompt(store.as_ref(), &repo, "now the ingest tags problem", "default");
        assert!(other.contains(&format!("(id {tags_id})")) && !other.contains(&format!("(id {hooks_id})")), "{other}");
    }

    #[test]
    fn pinned_notes_still_come_first_in_every_mode() {
        let (_dir, store, repo) = store();
        let pin = gotcha(store.as_ref(), &repo, "Always run tests on the server", "rule");
        store.set_curation(&repo, pin, agentops_graph::NodeProminence::Pinned, None).unwrap();
        for mode in ["default", "plan"] {
            assert!(prompt(store.as_ref(), &repo, "anything at all", mode).starts_with("## Pinned"), "{mode}");
        }
    }

    #[test]
    fn the_flow_tells_the_agent_which_path_to_pass_to_agentops_tools() {
        let (_dir, store, repo) = store();
        let with_hint = context(handle(store.as_ref(), &repo, "userpromptsubmit", &json!({ "prompt": "plan it", "session_id": "a", "permission_mode": "plan", "agentops_path": "conn-1" })).unwrap());
        assert!(with_hint.contains("Pass `path: \"conn-1\"`"), "{with_hint}");
        let without = context(handle(store.as_ref(), &repo, "userpromptsubmit", &json!({ "prompt": "plan it", "session_id": "b", "permission_mode": "plan" })).unwrap());
        assert!(without.contains(&format!("Pass `path: \"{repo}\"`")) && !without.contains("{AGENTOPS_PATH}"), "{without}");
    }

    #[test]
    fn a_task_notification_is_neither_recorded_nor_answered() {
        let (_dir, store, repo) = store();
        gotcha(store.as_ref(), &repo, "Hooks ignore the remote server", "hooks");
        let out = handle(store.as_ref(), &repo, "userpromptsubmit", &json!({ "prompt": "<task-notification>\n<summary>hooks agent finished</summary>", "session_id": "s", "permission_mode": "plan" })).unwrap();
        assert!(out.is_none());
        assert!(store.session_events(&repo, "s").unwrap().is_empty());
        let hand_back = "<agent-message from=\"a1\">\n[Subagent hand-back] the hooks report\n</agent-message>";
        assert!(handle(store.as_ref(), &repo, "userpromptsubmit", &json!({ "prompt": hand_back, "session_id": "s", "permission_mode": "plan" })).unwrap().is_none());
        assert!(store.session_events(&repo, "s").unwrap().is_empty());
        assert!(prompt(store.as_ref(), &repo, "plan the hooks work", "plan").contains("task-start flow"), "a notification never counts as the episode's first prompt");
    }

    #[test]
    fn last_prompt_reads_both_modes_and_ignores_notifications() {
        let (_dir, store, repo) = store();
        prompt(store.as_ref(), &repo, "first", "default");
        prompt(store.as_ref(), &repo, "second, planned", "plan");
        handle(store.as_ref(), &repo, "userpromptsubmit", &json!({ "prompt": "<task-notification>x", "session_id": "s" })).unwrap();
        assert_eq!(last_prompt(store.as_ref(), &repo, "s").unwrap().as_deref(), Some("second, planned"));
    }

    #[test]
    fn a_planning_subagent_gets_pins_and_a_seed_from_the_last_prompt_and_others_get_nothing() {
        let (_dir, store, repo) = store();
        let id = gotcha(store.as_ref(), &repo, "Hooks ignore the remote server", "hook capture opens the local store");
        prompt(store.as_ref(), &repo, "make the hooks follow the remote server", "plan");
        let recorded = || store.session_events(&repo, "s").unwrap().into_iter().filter(|e| e.event_kind == RECALL_INJECTED).count();
        let before = recorded();
        let explore = context(handle(store.as_ref(), &repo, "subagentstart", &json!({ "session_id": "s", "agent_type": "Explore", "agent_id": "a1" })).unwrap());
        assert!(explore.contains(&format!("(id {id})")) && explore.starts_with(&format!("Pass `path: \"{repo}\"`")), "{explore}");
        let council = context(handle(store.as_ref(), &repo, "subagentstart", &json!({ "session_id": "s", "agent_type": "plan-council-member", "agent_id": "a2", "agentops_path": "conn-1" })).unwrap());
        assert!(council.starts_with("First, before any Read, Grep or Bash") && council.contains("`path: \"conn-1\"`") && council.contains(&format!("(id {id})")), "{council}");
        assert!(council.chars().count() <= crate::budget::SUBAGENT_SEED_BUDGET);
        assert!(explore.chars().count() <= crate::budget::SUBAGENT_SEED_BUDGET);
        assert!(handle(store.as_ref(), &repo, "subagentstart", &json!({ "session_id": "s", "agent_type": "statusline-setup" })).unwrap().is_none());
        assert_eq!(recorded(), before, "a subagent's seed never counts as shown to the main session");
    }

    #[test]
    fn a_needs_review_memory_never_reaches_a_prompt_seed_a_subagent_seed_or_the_pins() {
        let (_dir, store, repo) = store();
        let id = gotcha(store.as_ref(), &repo, "Hooks ignore the remote server", "hook capture opens the local store");
        store.set_curation(&repo, id, agentops_graph::NodeProminence::NeedsReview, Some("unclassified on import")).unwrap();
        let plan = prompt(store.as_ref(), &repo, "make the hooks follow the remote server", "plan");
        let explore = context(handle(store.as_ref(), &repo, "subagentstart", &json!({ "session_id": "s", "agent_type": "Explore", "agent_id": "a1" })).unwrap());
        let later = prompt(store.as_ref(), &repo, "hooks remote server again", "default");
        for text in [&plan, &explore, &later] {
            assert!(!text.contains(&format!("(id {id})")), "{text}");
        }
    }

    fn present(store: &dyn GraphStore, repo: &str, plan: Option<&str>) -> Option<Value> {
        let input = plan.map(|p| json!({ "plan": p })).unwrap_or(json!({}));
        handle(store, repo, "pretooluse", &json!({ "session_id": "s", "tool_name": "ExitPlanMode", "tool_input": input })).unwrap()
    }

    /// A main-thread (or, with `agent`, subagent) tool call as Claude Code
    /// reports it to PostToolUse.
    fn tool_call(store: &dyn GraphStore, repo: &str, tool_name: &str, tool_input: Value, agent: Option<&str>) {
        let mut payload = json!({ "session_id": "s", "tool_name": tool_name, "tool_input": tool_input, "tool_response": "ok" });
        if let Some(agent) = agent {
            payload["agent_id"] = json!(agent);
            payload["agent_type"] = json!("plan-council-member");
        }
        handle(store, repo, "posttooluse", &payload).unwrap();
    }

    /// Everything the task-start flow asks for, from the main thread.
    fn run_flow(store: &dyn GraphStore, repo: &str) {
        tool_call(store, repo, "mcp__agentops__get_symbol", json!({ "name": "x" }), None);
        for skill in ["plan", "library-docs", "audit-plan"] {
            tool_call(store, repo, "Skill", json!({ "skill": skill }), None);
        }
    }

    #[test]
    fn a_plan_missing_a_section_is_sent_back_once_then_let_through_with_a_warning() {
        let (_dir, store, repo) = store();
        prompt(store.as_ref(), &repo, "plan something", "plan");
        run_flow(store.as_ref(), &repo);
        let incomplete = "# Plan\n\n## Research performed\n- checked\n";

        let first = present(store.as_ref(), &repo, Some(incomplete)).unwrap();
        assert_eq!(first["hookSpecificOutput"]["permissionDecision"], "deny");
        let reason = first["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("\"Council verdict\"") && reason.contains("audit-plan") && !reason.contains("\"Research performed\""), "{reason}");

        let second = present(store.as_ref(), &repo, Some(incomplete)).unwrap();
        assert!(second["hookSpecificOutput"].get("permissionDecision").is_none(), "never more than ENFORCE_MAX_NUDGES nudges: {second}");
        assert!(second["systemMessage"].as_str().unwrap().contains("Council verdict"), "{second}");

        // A new planning episode gets its own nudge, and needs its own evidence.
        prompt(store.as_ref(), &repo, "go", "default");
        prompt(store.as_ref(), &repo, "plan the next thing", "plan");
        assert_eq!(present(store.as_ref(), &repo, Some(incomplete)).unwrap()["hookSpecificOutput"]["permissionDecision"], "deny");
    }

    const COMPLETE: &str = "# P\n## Research performed\nx\n## Council verdict\nclear";

    #[test]
    fn a_plan_whose_episode_skipped_recall_or_a_flow_skill_is_sent_back_naming_what_is_missing() {
        let (_dir, store, repo) = store();
        prompt(store.as_ref(), &repo, "plan something", "plan");
        tool_call(store.as_ref(), &repo, "Skill", json!({ "skill": "audit-plan" }), None);

        let first = present(store.as_ref(), &repo, Some(COMPLETE)).unwrap();
        assert_eq!(first["hookSpecificOutput"]["permissionDecision"], "deny");
        let reason = first["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("recall") && reason.contains("the plan skill") && reason.contains("the library-docs skill") && !reason.contains("the audit-plan skill") && !reason.contains("section"), "{reason}");

        let second = present(store.as_ref(), &repo, Some(COMPLETE)).unwrap();
        assert!(second["hookSpecificOutput"].get("permissionDecision").is_none(), "{second}");
        assert!(second["systemMessage"].as_str().unwrap().contains("library-docs"), "{second}");
    }

    #[test]
    fn only_the_main_threads_recall_and_only_recall_tools_count_as_evidence() {
        let (_dir, store, repo) = store();
        prompt(store.as_ref(), &repo, "plan something", "plan");
        for skill in ["plan", "library-docs", "audit-plan"] {
            tool_call(store.as_ref(), &repo, "Skill", json!({ "skill": skill }), None);
        }
        // A council member's recall, and agentops tools that aren't recall.
        tool_call(store.as_ref(), &repo, "mcp__agentops__list_gotchas", json!({ "path": "r" }), Some("a1"));
        for tool in ["mcp__agentops__search_docs", "mcp__agentops__get_library", "mcp__agentops__get_docs"] {
            tool_call(store.as_ref(), &repo, tool, json!({}), None);
        }
        tool_call(store.as_ref(), &repo, "Skill", json!({ "skill": "session" }), None);
        let reason = present(store.as_ref(), &repo, Some(COMPLETE)).unwrap()["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().to_string();
        assert!(reason.contains("recall"), "{reason}");
        let kinds: Vec<String> = store.session_events(&repo, "s").unwrap().into_iter().map(|e| e.event_kind).collect();
        assert_eq!(kinds.iter().filter(|k| *k == RECALL_CALL).count(), 0, "{kinds:?}");
        assert_eq!(kinds.iter().filter(|k| *k == FLOW_SKILL).count(), 3, "{kinds:?}");
    }

    #[test]
    fn a_complete_plan_or_one_with_no_plan_text_goes_through_untouched() {
        let (_dir, store, repo) = store();
        run_flow(store.as_ref(), &repo);
        assert!(present(store.as_ref(), &repo, Some("# P\n## Research performed\nx\n### council VERDICT\nclear")).is_none());
        assert!(present(store.as_ref(), &repo, None).is_none(), "fail open when the plan text isn't in the payload");
        assert!(present(store.as_ref(), &repo, Some("Research performed and Council verdict mentioned only in prose")).is_some(), "only headings count");
    }

    #[test]
    fn a_write_into_claude_codes_memory_directory_is_redirected_to_add_note_and_nothing_else_is() {
        let (_dir, store, repo) = store();
        let pre = |tool: &str, path: &str| handle(store.as_ref(), &repo, "pretooluse", &json!({ "session_id": "s", "tool_name": tool, "tool_input": { "file_path": path } })).unwrap();
        for (tool, path) in [("Write", "/Users/me/.claude/projects/-Users-me-repo/memory/MEMORY.md"), ("Edit", "/Users/me/.claude/projects/-Users-me-repo/memory/feedback_x.md"), ("Write", r"C:\Users\me\.claude\projects\C--repo\memory\MEMORY.md")] {
            let out = pre(tool, path).unwrap_or_else(|| panic!("{tool} {path} must be denied"));
            assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
            assert!(out["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("add_note"));
        }
        assert!(pre("Read", "/Users/me/.claude/projects/-Users-me-repo/memory/MEMORY.md").is_none(), "reads stay allowed");
        assert!(pre("Write", "/Users/me/repo/src/memory/mod.rs").is_none());
        assert!(pre("Write", "/Users/me/.claude/projects/-Users-me-repo/notes.md").is_none());
        assert!(pre("Write", "/Users/me/.claude/plans/plan.md").is_none());
        assert_eq!(store.session_events(&repo, "s").unwrap().iter().filter(|e| e.event_kind == MEMORY_WRITE_REDIRECTED).count(), 3);
    }

    /// The server runs these hooks against Postgres, whose lexical search
    /// ANDs a multi-word query where SQLite ORs it. Skips when no Postgres
    /// is reachable; the server's CI sets AGENTOPS_TEST_DATABASE_URL.
    #[test]
    fn the_prompt_hook_records_and_recalls_against_postgres() {
        let url = std::env::var("AGENTOPS_TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:test@localhost:5433/agentops_test".to_string());
        let Ok(pg) = agentops_graph_pg::PostgresGraphStore::connect(&url) else {
            eprintln!("skipping the_prompt_hook_records_and_recalls_against_postgres: no Postgres reachable at {url}");
            return;
        };
        let repo = format!("hooks-pg-test-{}", now_nanos());
        let id = gotcha(&pg, &repo, "Hooks ignore the remote server", "hook capture opens the local store");
        let first = prompt(&pg, &repo, "make the hooks follow the remote server reliably", "plan");
        let events = pg.session_events(&repo, "s").unwrap();
        pg.wipe_repo(&repo).unwrap();
        assert!(first.contains("task-start flow") && first.contains(&format!("(id {id})")), "a multi-word prompt must still recall on Postgres: {first}");
        assert!(events.iter().any(|e| e.event_kind == "user_intent" && e.tool_name == PLAN_PROMPT_TOOL), "{events:?}");
    }

    #[test]
    fn hook_output_omits_absent_fields() {
        let out = hook_output("UserPromptSubmit", Some("ctx"), None);
        assert_eq!(out, json!({ "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": "ctx" } }));
    }
}
