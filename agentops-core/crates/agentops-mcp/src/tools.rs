//! Tool definitions and dispatch — driven from **one table** (`tool_specs`).
//! `main` gated write tools via `READ_ONLY_TOOLS`/`WRITE_TOOLS` name-only
//! arrays *plus* a separate `list_tools()` definition list *plus* a
//! separate `call_tool()` dispatch match — three independently-maintained
//! places, confirmed still in sync there but with zero test coverage that
//! would catch future drift. Here, `access: AccessMode` is just another
//! field on the same `ToolSpec` row `list_tools()`/`call_tool()` both
//! derive from — that class of bug can't exist because there's only one
//! table to update when a tool is added.

use std::path::{Path, PathBuf};

use agentops_graph::{GraphStore, NewTask, NodeKind, TaskStatus};
use serde_json::{json, Value};

use crate::protocol::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::scan::repo_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// Read-only tools only — the safe default for an untrusted or
    /// read-only integration.
    Advisor,
    /// Every tool, including ones that write to the graph store, the
    /// filesystem, or call an external (paid) API.
    Full,
}

type Handler = fn(&Value) -> anyhow::Result<String>;

struct ToolSpec {
    name: &'static str,
    description: &'static str,
    access: AccessMode,
    annotations: ToolAnnotations,
    input_schema: fn() -> Value,
    handler: Handler,
}

fn tool_specs() -> Vec<ToolSpec> {
    const READ_ONLY: ToolAnnotations = ToolAnnotations { read_only_hint: true, destructive_hint: false, idempotent_hint: true, open_world_hint: false };
    const WRITE_IDEMPOTENT: ToolAnnotations = ToolAnnotations { read_only_hint: false, destructive_hint: false, idempotent_hint: true, open_world_hint: false };

    vec![
        ToolSpec {
            name: "status",
            description: "Reports the latest scan summary for a repo (files/symbols added/changed/removed).",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            handler: tool_status,
        },
        ToolSpec {
            name: "list_gotchas",
            description: "Lists every Gotcha node recorded for a repo. Pass session_id to correlate this call into a cross-tool activity feed (see get_session) and count it toward the usage dashboard's knowledge-reuse tracking.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "session_id": { "type": "string" } }, "required": ["path"] }),
            handler: tool_list_gotchas,
        },
        ToolSpec {
            name: "get_symbol",
            description: "Looks up a symbol by name (optionally disambiguated by file path). Pass session_id to correlate this call into a cross-tool activity feed (see get_session) and count it toward the usage dashboard's knowledge-reuse tracking.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || {
                json!({ "type": "object", "properties": { "path": { "type": "string" }, "name": { "type": "string" }, "file": { "type": "string" }, "session_id": { "type": "string" } }, "required": ["path", "name"] })
            },
            handler: tool_get_symbol,
        },
        ToolSpec {
            // Renamed from `get_changelog` (a stale name, always a
            // mismatch with this tool's actual "recent scans" behavior) —
            // also resolves a real name collision with `docbrain-mcp`'s
            // own `get_changelog` tool (library version-to-version
            // changelog entries, an unrelated concept) once both tool
            // tables are merged into one dispatcher.
            name: "list_scans",
            description: "Lists recent scans for a repo, most recent first.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "limit": { "type": "integer" } }, "required": ["path"] }),
            handler: tool_get_changelog,
        },
        ToolSpec {
            name: "scan_repo",
            description: "Scans the repo at `path` and persists it to the graph store — token-bounded change detection (Added/Changed/Removed), safe to call repeatedly. Set with_embeddings to also make new/changed symbols findable via semantic_search (local, no API cost, but real CPU latency — off by default). Pass session_id to correlate this call into a cross-tool activity feed (see get_session).",
            access: AccessMode::Full,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "with_embeddings": { "type": "boolean" }, "session_id": { "type": "string" } }, "required": ["path"] }),
            handler: tool_scan_repo,
        },
        ToolSpec {
            name: "add_note",
            description: "Writes a new note (gotcha/decision/knowledge) to the repo's notes folder and ingests it into the graph in one step — the write-back tool for an agent that just learned something worth remembering. Omit note_type to let it be classified automatically. Set with_embeddings to make it findable via semantic_search. Pass session_id to correlate this call into a cross-tool activity feed (see get_session).",
            // `Advisor`, not `Full` -- deliberately available in both access
            // modes, unlike every other write tool. Advisor mode exists to
            // block destructive/costly actions (bulk rescans, paid LLM
            // calls via explain_symbol); growing the knowledge base by
            // recording a gotcha/decision is neither, and gating it behind
            // `Full` would defeat the point of an advisor that's supposed
            // to keep getting smarter across sessions even when an org
            // hasn't opted a caller into write access generally.
            access: AccessMode::Advisor,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "title": { "type": "string" },
                        "body": { "type": "string" },
                        "note_type": { "type": "string", "enum": ["gotcha", "decision", "knowledge"] },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "with_embeddings": { "type": "boolean" },
                        "session_id": { "type": "string" },
                    },
                    "required": ["path", "title", "body"],
                })
            },
            handler: tool_add_note,
        },
        ToolSpec {
            name: "ingest_notes",
            description: "Walks a notes folder (a real vault or an unorganized one) and ingests every note into the graph — classifying freeform notes with no frontmatter/folder signal via the heuristic classifier. Set with_embeddings to make every note findable via semantic_search. Pass session_id to correlate this call into a cross-tool activity feed (see get_session).",
            // `Advisor`, same reasoning as `add_note` just above.
            access: AccessMode::Advisor,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || {
                json!({ "type": "object", "properties": { "path": { "type": "string" }, "notes_path": { "type": "string" }, "with_embeddings": { "type": "boolean" }, "session_id": { "type": "string" } }, "required": ["path"] })
            },
            handler: tool_ingest_notes,
        },
        ToolSpec {
            name: "explain_symbol",
            description: "Explains a symbol via the Anthropic API and persists the result as a Definition node linked to it. Requires AGENTOPS_ANTHROPIC_API_KEY. Costs a real API call — never run automatically during a scan. Pass session_id to correlate this call into a cross-tool activity feed (see get_session).",
            access: AccessMode::Full,
            annotations: ToolAnnotations { read_only_hint: false, destructive_hint: false, idempotent_hint: false, open_world_hint: true },
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "symbol_id": { "type": "integer" }, "session_id": { "type": "string" } }, "required": ["path", "symbol_id"] }),
            handler: tool_explain_symbol,
        },
        ToolSpec {
            name: "related_context",
            description: "Pattern completion around a symbol (Initiative 4, CLS-inspired retrieval plan): finds symbols elsewhere in the repo that are similar (dense embedding, requires with_embeddings from an earlier scan) or graph-connected (Personalized PageRank over Affects/References edges) to the given symbol, and returns each one's own recorded Gotcha/Decision notes. Read-only, no LLM call — the same recombined context explain_symbol now folds into its prompt automatically, exposed directly so an agent session can ask 'what's associated with this symbol' without triggering a full explanation. Pass session_id to correlate this call into a cross-tool activity feed (see get_session) and count it toward the usage dashboard's knowledge-reuse tracking.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "symbol_id": { "type": "integer" }, "top_k": { "type": "integer" }, "session_id": { "type": "string" } }, "required": ["path", "symbol_id"] }),
            handler: tool_related_context,
        },
        ToolSpec {
            name: "get_session",
            description: "Returns the correlated cross-tool activity feed for one session_id in a repo — every scan_repo/add_note/ingest_notes/explain_symbol call that was made with that same session_id, oldest first. Empty if session_id was never passed to any write tool for this repo.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "session_id": { "type": "string" } }, "required": ["path", "session_id"] }),
            handler: tool_get_session,
        },
        ToolSpec {
            name: "end_session",
            description: "Signals that an agent session is done (Initiative 5, CLS-inspired retrieval plan) and triggers embedding consolidation: trains a small per-repo projection head on top of the frozen base embeddings, using this repo's own plasticity-shaped Affects/References edges as a replay buffer, and promotes it only if it doesn't regress retrieval quality against whatever's currently active. Requires session_id to have been passed to at least one earlier write tool call in this repo (scan_repo/add_note/ingest_notes/explain_symbol) — otherwise there's no recorded activity to confirm consolidation is warranted. No-ops gracefully (never errors) if there isn't enough plasticity signal yet. Real, if modest, CPU cost — call once at the natural end of a work session, not after every tool call.",
            access: AccessMode::Full,
            annotations: ToolAnnotations { read_only_hint: false, destructive_hint: false, idempotent_hint: false, open_world_hint: false },
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "session_id": { "type": "string" } }, "required": ["path", "session_id"] }),
            handler: tool_end_session,
        },
        ToolSpec {
            name: "create_task",
            description: "Creates a native task owned by AgentOps (not synced to Linear). Pass session_id to correlate future scan_repo/add_note/etc. calls into this task's activity feed via get_task_activity.",
            access: AccessMode::Full,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "priority": { "type": "string" },
                        "assignee": { "type": "string" },
                        "session_id": { "type": "string" },
                    },
                    "required": ["path", "title"],
                })
            },
            handler: tool_create_task,
        },
        ToolSpec {
            name: "list_tasks",
            description: "Lists every task (native or Linear-synced) recorded for a repo.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            handler: tool_list_tasks,
        },
        ToolSpec {
            name: "update_task_status",
            description: "Moves a task to a new status (todo, in_progress, in_review, done, cancelled).",
            access: AccessMode::Full,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "task_id": { "type": "integer" },
                        "status": { "type": "string", "enum": ["todo", "in_progress", "in_review", "done", "cancelled"] },
                    },
                    "required": ["path", "task_id", "status"],
                })
            },
            handler: tool_update_task_status,
        },
        ToolSpec {
            name: "get_task_activity",
            description: "The task's built-in final-audit view: every session_events row correlated under the task's session_id (see create_task's session_id), oldest first. Empty if the task has no session_id or no activity was recorded under it.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "task_id": { "type": "integer" } }, "required": ["path", "task_id"] }),
            handler: tool_get_task_activity,
        },
        ToolSpec {
            name: "init_agents_md",
            description: "Writes (or refreshes) AGENTS.md for this repo with a resolved NOTES_PATH, and ensures .gitignore excludes generated scan output. Lets an agent bootstrap the write-back protocol for itself over MCP, without needing shell access to the CLI's `install` command.",
            access: AccessMode::Full,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "notes_path": { "type": "string" } }, "required": ["path"] }),
            handler: tool_init_agents_md,
        },
        ToolSpec {
            name: "generate_docs",
            description: "Renders this repo's onboarding/engineering doc (repo-map.md) from the indexed graph — stats, a ranked file/symbol map, and every gotcha/decision with the symbol it affects. Requires the repo to have been scanned already.",
            access: AccessMode::Full,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            handler: tool_generate_docs,
        },
        ToolSpec {
            // Renamed from `semantic_search` — collides with
            // `agentops-heavy-mcp`'s own `semantic_search` tool (a
            // genuinely different backend: Qdrant-backed, requires
            // `semantic_index` to have run first) once both tool tables
            // are merged into one dispatcher. Heavy's keeps the shorter
            // name, consistent with the REST-layer precedent (heavy's
            // meaning wins the clean path on a collision).
            name: "local_semantic_search",
            description: "Dense-vector search over whatever symbols/gotchas/decisions/notes have been embedded (see scan_repo/add_note/ingest_notes's with_embeddings flag) — complements get_symbol's exact-name lookup with 'find something like this' search. Only returns hits among nodes that were actually embedded; nothing is embedded by default. Set mode to 'hybrid' to fuse in lexical (keyword/BM25) and exact-name-match signals too — no embedding required for a node to be found via those signals, and a literal function-name query reliably surfaces it even when nothing was ever embedded. With mode 'hybrid', set graph_expand to also spread activation from the fused top hits across Affects/References edges (Personalized PageRank) so graph-connected results can outrank a purely textual/semantic match — off by default. Set mode to 'gist_then_detail' for two-tier retrieval: first matches the repo's generated documentation sections (the compressed 'gist' of a module/repo), then searches only the symbols/gotchas/decisions those matched sections actually cover — sharper results for a broad/module-level query, at the cost of ignoring kind filtering and anything outside a matched section's coverage (falls back to an unscoped search if no section matches).",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "query": { "type": "string" },
                        "top_k": { "type": "integer" },
                        "kind": { "type": "string", "enum": ["symbol", "file", "gotcha", "decision", "note", "definition"] },
                        "mode": { "type": "string", "enum": ["dense", "hybrid", "gist_then_detail"] },
                        "graph_expand": { "type": "boolean" },
                    },
                    "required": ["path", "query"],
                })
            },
            handler: tool_semantic_search,
        },
    ]
}

pub fn list_tools(mode: AccessMode) -> Vec<ToolDefinition> {
    tool_specs()
        .into_iter()
        .filter(|s| mode == AccessMode::Full || s.access == AccessMode::Advisor)
        .map(|s| ToolDefinition { name: s.name, description: s.description, input_schema: (s.input_schema)(), annotations: s.annotations })
        .collect()
}

pub fn call_tool(mode: AccessMode, name: &str, args: &Value) -> Result<CallToolResult, String> {
    let specs = tool_specs();
    let Some(spec) = specs.iter().find(|s| s.name == name) else {
        return Err(format!("unknown tool '{name}'"));
    };
    // Defensive backstop, same reasoning `main` documented for its
    // equivalent re-check: `list_tools` already omits Full-only tools from
    // Advisor's listing structurally, but a client that calls a tool it was
    // never shown must still be refused here, not just left unlisted.
    if mode == AccessMode::Advisor && spec.access == AccessMode::Full {
        return Ok(CallToolResult::error(format!("tool '{name}' requires Full access mode")));
    }

    Ok(match (spec.handler)(args) {
        Ok(text) => CallToolResult::success(text),
        // `{e:#}` (anyhow's alternate Display), not `{e}` -- plain `{}`
        // only shows the outermost error's own message, which for a
        // `tokio_postgres::Error` wrapping `Kind::Db` is literally just
        // the four-word string "db error" with no detail at all (that
        // crate's own `Display` impl for the error *kind*, not the
        // underlying `DbError` payload, which only surfaces via the
        // `source()` chain). `{:#}` walks the whole chain, showing every
        // `.context(...)` layer down to the real cause. Caught live: a
        // genuine Postgres-side failure was completely invisible behind
        // "db error" with nothing to actually debug from.
        Err(e) => CallToolResult::error(format!("{e:#}")),
    })
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn get_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn repo_context(args: &Value) -> anyhow::Result<(Box<dyn GraphStore>, String)> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let path = Path::new(path_str);
    let store = crate::store::open_store(path)?;
    Ok((store, repo_name(path)))
}

/// AgentOps's MCP server is now registered once, globally, for every repo
/// on the machine (no per-repo `connect` step marks a repo as "known"
/// anymore) — so a repo an agent has never `scan_repo`'d against looks
/// identical to one that's genuinely empty: `open_store` silently
/// auto-creates an empty `.context/graph.db` either way. Read tools that
/// would otherwise return a bare "nothing found" call this to distinguish
/// the two and nudge the agent toward `scan_repo` instead of concluding
/// there's just nothing to know yet.
fn never_scanned_hint(store: &dyn GraphStore, repo: &str) -> anyhow::Result<Option<&'static str>> {
    Ok(if store.latest_scan(repo)?.is_none() { Some(" (this repo has never been scanned — call scan_repo first, then retry)") } else { None })
}

/// Module 6/8 (cross-tool session correlation + knowledge-reuse tracking):
/// every write tool that accepts an optional `session_id` calls this after
/// its own write succeeds, tagged `event_kind: "activity"`; a read tool
/// that actually returned an existing node calls it tagged `event_kind:
/// "hit"` with that node's id. A missing/absent `session_id` is a normal,
/// expected case (most callers don't correlate sessions) — a silent no-op,
/// not an error. Deliberately one helper for both kinds, not two — a
/// prior project's near-identical usage-logging feature let its "activity"
/// and "hit" recording paths diverge into separate helpers with different
/// call signatures, which caused duplicate logging calls and type
/// conflicts; collapsing into one signature here avoids that class of bug
/// entirely rather than fixing it after the fact.
fn maybe_record_session_event(path: &Path, args: &Value, tool_name: &str, description: &str, node_id: Option<i64>, event_kind: &str) -> anyhow::Result<()> {
    if let Some(session_id) = get_str(args, "session_id") {
        let store = crate::store::open_store(path)?;
        store.record_session_event(&repo_name(path), session_id, tool_name, description, node_id, event_kind)?;
    }
    Ok(())
}

fn tool_status(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    match store.latest_scan(&repo)? {
        Some(scan) => {
            let mut out = format!(
                "repo: {repo}\nlast scan: {}\nfiles: +{} ~{} -{}\nsymbols: +{} ~{} -{}",
                scan.started_at, scan.files_added, scan.files_changed, scan.files_removed, scan.symbols_added, scan.symbols_changed, scan.symbols_removed
            );
            out.push_str(&render_repo_state_section(store.as_ref(), &repo)?);
            Ok(out)
        }
        None => Ok(format!("repo: {repo}\nno scans recorded yet — call scan_repo first")),
    }
}

/// `get_repo_state` returning `None` is a real, expected state (a repo
/// scanned before this feature existed, or one with zero gotchas/decisions
/// ever recorded) — omit the section entirely rather than erroring or
/// printing an empty one.
fn render_repo_state_section(store: &dyn GraphStore, repo: &str) -> anyhow::Result<String> {
    let Some(state) = store.get_repo_state(repo)? else {
        return Ok(String::new());
    };
    if state.top_gotcha_ids.is_empty() && state.top_decision_ids.is_empty() {
        return Ok(String::new());
    }

    let names = |ids: &[i64]| -> anyhow::Result<Vec<String>> {
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some(node) = store.get_node(repo, id)? {
                out.push(node.name.unwrap_or_else(|| "(untitled)".to_string()));
            }
        }
        Ok(out)
    };

    let mut out = String::new();
    if !state.top_gotcha_ids.is_empty() {
        out.push_str(&format!("\ntop gotchas: {}", names(&state.top_gotcha_ids)?.join(", ")));
    }
    if !state.top_decision_ids.is_empty() {
        out.push_str(&format!("\ntop decisions: {}", names(&state.top_decision_ids)?.join(", ")));
    }
    Ok(out)
}

fn tool_list_gotchas(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let mut gotchas = store.nodes_by_kind(&repo, NodeKind::Gotcha)?;
    if gotchas.is_empty() {
        let hint = never_scanned_hint(store.as_ref(), &repo)?.unwrap_or_default();
        return Ok(format!("No gotchas recorded.{hint}"));
    }
    if let Some(path_str) = get_str(args, "path") {
        maybe_record_session_event(Path::new(path_str), args, "list_gotchas", &format!("listed {} gotcha(s)", gotchas.len()), None, "hit")?;
    }
    // Full-prominence first -- every gotcha is still listed (this is
    // permanent knowledge, never hidden), just ranked so a curated-down
    // one doesn't dominate an agent's attention over ones nobody's
    // demoted.
    gotchas.sort_by_key(|n| n.prominence == agentops_graph::NodeProminence::Reduced);
    Ok(gotchas
        .iter()
        .map(|n| {
            let reduced = if n.prominence == agentops_graph::NodeProminence::Reduced {
                format!(" ⚠ reduced prominence — {}", n.curation_reason.as_deref().unwrap_or("no reason recorded"))
            } else {
                String::new()
            };
            format!("- {} (node {}){reduced}", n.name.as_deref().unwrap_or("(untitled)"), n.id)
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn tool_get_symbol(args: &Value) -> anyhow::Result<String> {
    use agentops_graph::EdgeRelation;

    let (store, repo) = repo_context(args)?;
    let name = get_str(args, "name").ok_or_else(|| anyhow::anyhow!("missing required 'name'"))?;
    let file = get_str(args, "file").map(Path::new);
    let id = match agentops_llm::find_symbol_by_name(store.as_ref(), &repo, name, file) {
        Ok(id) => id,
        Err(e) => {
            let hint = never_scanned_hint(store.as_ref(), &repo)?.unwrap_or_default();
            anyhow::bail!("{e}{hint}");
        }
    };
    let node = store.get_node(&repo, id)?.ok_or_else(|| anyhow::anyhow!("symbol resolved but node #{id} not found"))?;
    if let Some(path_str) = get_str(args, "path") {
        maybe_record_session_event(Path::new(path_str), args, "get_symbol", &format!("looked up symbol {name}"), Some(id), "hit")?;
    }

    let mut out = format!(
        "{} ({}) — {}:{}-{}\n\n{}",
        node.name.as_deref().unwrap_or(name),
        "symbol",
        node.path.as_deref().unwrap_or("?"),
        node.start_line.unwrap_or(0),
        node.end_line.unwrap_or(0),
        node.content.as_deref().unwrap_or("")
    );

    // Codebrain-2's payoff: a gotcha/decision recorded against this exact
    // symbol should resurface right here, where an agent is actually about
    // to touch the code — not just in the full generated repo-map doc.
    let affecting: Vec<_> = store.edges_to(&repo, id)?.into_iter().filter(|e| e.relation == EdgeRelation::Affects).collect();
    // Resolved once (not once for scoring, again for rendering) into a
    // lookup both the sort and the render loop below share.
    let notes_by_src: std::collections::HashMap<i64, agentops_graph::Node> = affecting.iter().filter_map(|e| store.get_node(&repo, e.src_id).ok().flatten().map(|n| (e.src_id, n))).collect();
    let mut affecting = affecting;
    // Most-relevant first — reinforced, recently-matched gotchas/decisions
    // outrank ones nobody's confirmed still apply; a curated-down note is
    // further damped so it doesn't outrank an un-curated one on weight alone.
    affecting.sort_by(|a, b| {
        let dampen = |edge: &agentops_graph::Edge| notes_by_src.get(&edge.src_id).map(|n| agentops_graph::prominence_rank_multiplier(n.prominence)).unwrap_or(1.0);
        let score_a = agentops_graph::effective_weight(a.weight, agentops_graph::age_days(&a.updated_at)) * dampen(a);
        let score_b = agentops_graph::effective_weight(b.weight, agentops_graph::age_days(&b.updated_at)) * dampen(b);
        score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    if !affecting.is_empty() {
        // Hallucination-prevention (bi-temporal versioning's actual payoff,
        // not optional — see module-2's design doc): a gotcha/decision is
        // only trustworthy if the symbol hasn't changed since it was last
        // confirmed relevant. `edge.updated_at` is bumped every time
        // `connect_many` re-matches/reinforces this note against this
        // symbol (including on every rescan) — if the symbol has a version
        // whose `valid_from` is *newer* than that, the code changed after
        // the note was last confirmed, and the note might be stale. Plain
        // string comparison is safe here: both timestamps always come from
        // the same backend in the same format within one comparison.
        let symbol_history = store.node_history(id)?;
        let is_possibly_stale = |edge_updated_at: &str| symbol_history.iter().any(|v| v.valid_from.as_str() > edge_updated_at);

        let mut gotchas = Vec::new();
        let mut decisions = Vec::new();
        for edge in &affecting {
            if let Some(note) = notes_by_src.get(&edge.src_id) {
                let staleness = if is_possibly_stale(&edge.updated_at) { " ⚠ possibly stale — this symbol changed since this note was last confirmed relevant" } else { "" };
                let reduced = if note.prominence == agentops_graph::NodeProminence::Reduced {
                    format!(" ⚠ reduced prominence — {}", note.curation_reason.as_deref().unwrap_or("no reason recorded"))
                } else {
                    String::new()
                };
                let entry = format!("- {}: {}{}{}", note.name.as_deref().unwrap_or("(untitled)"), note.content.as_deref().unwrap_or(""), staleness, reduced);
                match note.kind {
                    NodeKind::Gotcha => gotchas.push(entry),
                    NodeKind::Decision => decisions.push(entry),
                    _ => {}
                }
            }
        }
        if !gotchas.is_empty() {
            out.push_str(&format!("\n\n## Known gotchas affecting this symbol\n\n{}", gotchas.join("\n")));
        }
        if !decisions.is_empty() {
            out.push_str(&format!("\n\n## Decisions affecting this symbol\n\n{}", decisions.join("\n")));
        }
    }

    Ok(out)
}

fn tool_get_changelog(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let scans = store.list_scans(&repo)?;
    if scans.is_empty() {
        return Ok("No scans recorded.".to_string());
    }
    Ok(scans
        .into_iter()
        .take(limit)
        .map(|s| format!("{}: files +{}~{}-{} symbols +{}~{}-{}", s.started_at, s.files_added, s.files_changed, s.files_removed, s.symbols_added, s.symbols_changed, s.symbols_removed))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn tool_scan_repo(args: &Value) -> anyhow::Result<String> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let summary = crate::scan::scan_and_persist(Path::new(path_str), get_bool(args, "with_embeddings"))?;
    let description = format!(
        "scanned: {} files, {} symbols, {} dependency edges ({} files pruned, {} symbols pruned)",
        summary.files, summary.symbols, summary.dependency_edges, summary.pruned_files, summary.pruned_symbols
    );
    maybe_record_session_event(Path::new(path_str), args, "scan_repo", &description, None, "activity")?;
    Ok(format!(
        "Scanned {}: {} files, {} symbols, {} dependency edges ({} files pruned, {} symbols pruned)",
        path_str, summary.files, summary.symbols, summary.dependency_edges, summary.pruned_files, summary.pruned_symbols
    ))
}

fn tool_add_note(args: &Value) -> anyhow::Result<String> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let title = get_str(args, "title").ok_or_else(|| anyhow::anyhow!("missing required 'title'"))?;
    let body = get_str(args, "body").ok_or_else(|| anyhow::anyhow!("missing required 'body'"))?;
    let note_type_str = get_str(args, "note_type");
    let tags: Vec<String> = args.get("tags").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let explicit_notes_path = get_str(args, "notes_path").map(PathBuf::from);

    let note_type = match note_type_str {
        Some("gotcha") => Some(agentops_notes::NoteType::Gotcha),
        Some("decision") => Some(agentops_notes::NoteType::Decision),
        Some("knowledge") => Some(agentops_notes::NoteType::Knowledge),
        None => None,
        Some(other) => anyhow::bail!("invalid note_type '{other}', expected gotcha, decision, or knowledge"),
    };

    let result = crate::notes::add_note(Path::new(path_str), title, body, note_type, &tags, explicit_notes_path.as_deref(), get_bool(args, "with_embeddings"))?;
    let type_str = match result.note_type {
        agentops_notes::NoteType::Gotcha => "gotcha",
        agentops_notes::NoteType::Decision => "decision",
        agentops_notes::NoteType::Knowledge => "knowledge",
        agentops_notes::NoteType::Context => "context",
    };
    maybe_record_session_event(Path::new(path_str), args, "add_note", &format!("added {type_str} note: {title}"), None, "activity")?;
    Ok(format!("Wrote {} ({type_str}) and ingested it ({} edge(s) to related symbols, {} reinforced).", result.file_path.display(), result.edges_written, result.edges_reinforced))
}

fn tool_ingest_notes(args: &Value) -> anyhow::Result<String> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let explicit_notes_path = get_str(args, "notes_path").map(PathBuf::from);

    let classifier = agentops_notes::HeuristicClassifier;
    let matcher = agentops_notes::WordBoundaryMatcher::default();
    let summary = crate::notes::ingest_notes_dir(Path::new(path_str), explicit_notes_path.as_deref(), &classifier, &matcher, get_bool(args, "with_embeddings"))?;

    let notes_dir = agentops_notes::resolve_notes_path(Path::new(path_str), explicit_notes_path.as_deref());
    maybe_record_session_event(Path::new(path_str), args, "ingest_notes", &format!("ingested {} note(s) from {}", summary.notes_written, notes_dir.display()), None, "activity")?;
    Ok(format!("Ingested {} note(s) from {}, wrote {} edge(s), reinforced {}.", summary.notes_written, notes_dir.display(), summary.edges_written, summary.edges_reinforced))
}

fn tool_init_agents_md(args: &Value) -> anyhow::Result<String> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let notes_path = get_str(args, "notes_path").map(PathBuf::from);
    let result = crate::init::init_agents_md(Path::new(path_str), notes_path.as_deref())?;
    Ok(format!("Wrote {} (NOTES_PATH: {})", result.agents_md_path.display(), result.notes_path.display()))
}

fn tool_generate_docs(args: &Value) -> anyhow::Result<String> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let out_path = crate::docgen::generate_docs(Path::new(path_str))?;
    Ok(format!("Wrote {}", out_path.display()))
}

fn parse_node_kind(s: &str) -> Option<NodeKind> {
    match s {
        "symbol" => Some(NodeKind::Symbol),
        "file" => Some(NodeKind::File),
        "gotcha" => Some(NodeKind::Gotcha),
        "decision" => Some(NodeKind::Decision),
        "note" => Some(NodeKind::Note),
        "definition" => Some(NodeKind::Definition),
        _ => None,
    }
}

fn tool_semantic_search(args: &Value) -> anyhow::Result<String> {
    use agentops_embeddings::Embedder;

    let (store, repo) = repo_context(args)?;
    let query = get_str(args, "query").ok_or_else(|| anyhow::anyhow!("missing required 'query'"))?;
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let kind = match get_str(args, "kind") {
        Some(k) => Some(parse_node_kind(k).ok_or_else(|| anyhow::anyhow!("invalid kind '{k}'"))?),
        None => None,
    };

    // "gist_then_detail" (Initiative 2, CLS-inspired retrieval plan):
    // two-tier retrieval over agentops-docgen's already-compressed
    // NodeKind::DocSection "gist" tier, then a detail-tier search scoped to
    // what the matched section(s) actually cover. `kind` doesn't apply to
    // this mode -- the gist pass is always DocSection-only by construction,
    // the detail pass is always unfiltered by kind.
    if get_str(args, "mode") == Some("gist_then_detail") {
        let hits = agentops_retrieval::search_gist_then_detail(store.as_ref(), &agentops_embeddings::LocalEmbedder, &repo, query, top_k)?;
        if hits.is_empty() {
            return Ok("No matches.".to_string());
        }
        return Ok(hits
            .iter()
            .map(|h| format!("- {:?} {} (score {:.4}){}", h.node.kind, h.node.name.as_deref().unwrap_or("(untitled)"), h.fused_score, h.node.path.as_deref().map(|p| format!(" — {p}")).unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("\n"));
    }

    // "hybrid" (Phase 4): fuses dense + lexical + exact-match via
    // Reciprocal Rank Fusion (agentops_retrieval::search_hybrid) — finds
    // an exact function-name query dense-only search would miss, and gives
    // keyword-heavy queries real BM25 relevance, not just embedding
    // similarity. Default stays "dense" (today's original behavior) so
    // existing callers/tests see no change unless they opt in.
    if get_str(args, "mode") == Some("hybrid") {
        // `graph_expand` (Initiative 0, CLS-inspired retrieval plan): seeds
        // a bounded, in-memory Personalized PageRank from the RRF-fused top
        // hits and folds each node's graph-connectedness into the sort key.
        // Off by default so existing callers see no behavior change.
        let graph_expand = get_bool(args, "graph_expand");
        // Initiative 5: if this repo has a promoted projection head, the
        // dense signal re-ranks through it instead of raw base-embedding
        // similarity. `None` for a never-consolidated repo, transparently
        // falling back to today's unprojected ranking.
        let projector = crate::consolidate::load_active_projector(&repo);
        let hits = agentops_retrieval::search_hybrid(store.as_ref(), &agentops_embeddings::LocalEmbedder, &repo, query, top_k, kind, graph_expand, projector.as_ref().map(|p| p as &dyn agentops_retrieval::EmbeddingProjector))?;
        if hits.is_empty() {
            return Ok("No matches.".to_string());
        }
        return Ok(hits
            .iter()
            .map(|h| {
                let signals = [h.dense_rank.map(|_| "dense"), h.lexical_rank.map(|_| "lexical"), h.exact_rank.map(|_| "exact")].into_iter().flatten().collect::<Vec<_>>().join("+");
                let graph = h.graph_score.filter(|s| *s > 0.0).map(|s| format!(", graph {s:.4}")).unwrap_or_default();
                let reduced = if h.node.prominence == agentops_graph::NodeProminence::Reduced {
                    format!(" ⚠ reduced prominence — {}", h.node.curation_reason.as_deref().unwrap_or("no reason recorded"))
                } else {
                    String::new()
                };
                format!(
                    "- {:?} {} (score {:.4}, signals: {signals}{graph}){}{reduced}",
                    h.node.kind,
                    h.node.name.as_deref().unwrap_or("(untitled)"),
                    h.fused_score,
                    h.node.path.as_deref().map(|p| format!(" — {p}")).unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let embedding = agentops_embeddings::LocalEmbedder.embed(query)?;
    // Fetch a buffer beyond top_k so re-ranking by curation has real hits
    // to reorder against, same buffer-then-truncate shape search_hybrid
    // already uses -- otherwise a curated-down node either never entered
    // the result set at all, or got truncated away before it could be
    // pushed down past ones that would otherwise rank just below it.
    let fetch_k = (top_k * 3).max(10);
    let mut hits = store.search_similar(&repo, &embedding, fetch_k, kind)?;
    if hits.is_empty() {
        return Ok("No matches (nothing embedded yet, or nothing close enough — see scan_repo/add_note/ingest_notes's with_embeddings flag).".to_string());
    }
    // Lower distance is better, so a Reduced node's *effective* distance is
    // pushed up (divided by the multiplier), not its raw distance changed --
    // the displayed `distance` below stays the real, undamped value.
    hits.sort_by(|(node_a, distance_a), (node_b, distance_b)| {
        let rank_a = distance_a / agentops_graph::prominence_rank_multiplier(node_a.prominence) as f32;
        let rank_b = distance_b / agentops_graph::prominence_rank_multiplier(node_b.prominence) as f32;
        rank_a.partial_cmp(&rank_b).unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(top_k);

    Ok(hits
        .iter()
        .map(|(n, distance)| {
            let reduced = if n.prominence == agentops_graph::NodeProminence::Reduced {
                format!(" ⚠ reduced prominence — {}", n.curation_reason.as_deref().unwrap_or("no reason recorded"))
            } else {
                String::new()
            };
            format!("- {:?} {} (distance {distance:.4}){}{reduced}", n.kind, n.name.as_deref().unwrap_or("(untitled)"), n.path.as_deref().map(|p| format!(" — {p}")).unwrap_or_default())
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn tool_get_session(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let session_id = get_str(args, "session_id").ok_or_else(|| anyhow::anyhow!("missing required 'session_id'"))?;
    let events = store.session_events(&repo, session_id)?;
    if events.is_empty() {
        return Ok(format!("No activity recorded for session '{session_id}' in {repo}."));
    }
    Ok(events.iter().map(|e| format!("- [{}] {}: {}", e.created_at, e.tool_name, e.description)).collect::<Vec<_>>().join("\n"))
}

fn tool_end_session(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let session_id = get_str(args, "session_id").ok_or_else(|| anyhow::anyhow!("missing required 'session_id'"))?;

    let events = store.session_events(&repo, session_id)?;
    if events.is_empty() {
        return Ok(format!("No activity recorded for session '{session_id}' in {repo} — nothing to consolidate."));
    }

    let report = crate::consolidate::run_embedding_consolidation(store.as_ref(), &repo)?;
    if !report.attempted {
        return Ok(format!("Consolidation skipped: {}", report.reason));
    }
    Ok(format!(
        "Consolidation ran on {} example(s) (candidate recall@{} {:.3} vs. baseline {:.3}): {}",
        report.examples_used,
        agentops_embeddings_train::RECALL_K,
        report.candidate_recall,
        report.baseline_recall,
        report.reason
    ))
}

fn parse_task_status(s: &str) -> Option<TaskStatus> {
    match s {
        "todo" => Some(TaskStatus::Todo),
        "in_progress" => Some(TaskStatus::InProgress),
        "in_review" => Some(TaskStatus::InReview),
        "done" => Some(TaskStatus::Done),
        "cancelled" => Some(TaskStatus::Cancelled),
        _ => None,
    }
}

fn tool_create_task(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let title = get_str(args, "title").ok_or_else(|| anyhow::anyhow!("missing required 'title'"))?;
    let task = NewTask {
        repo,
        title: title.to_string(),
        description: get_str(args, "description").map(String::from),
        status: TaskStatus::Todo,
        priority: get_str(args, "priority").map(String::from),
        assignee: get_str(args, "assignee").map(String::from),
        external_source: None,
        external_id: None,
        session_id: get_str(args, "session_id").map(String::from),
    };
    let id = store.create_task(task)?;
    Ok(format!("Created task {id}: {title}"))
}

fn tool_list_tasks(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let tasks = store.list_tasks(&repo)?;
    if tasks.is_empty() {
        return Ok("No tasks recorded.".to_string());
    }
    Ok(tasks.iter().map(|t| format!("- [{}] task {}: {}", t.status.as_db_str(), t.id, t.title)).collect::<Vec<_>>().join("\n"))
}

fn tool_update_task_status(args: &Value) -> anyhow::Result<String> {
    let (store, _repo) = repo_context(args)?;
    let task_id = args.get("task_id").and_then(|v| v.as_i64()).ok_or_else(|| anyhow::anyhow!("missing required 'task_id'"))?;
    let status_str = get_str(args, "status").ok_or_else(|| anyhow::anyhow!("missing required 'status'"))?;
    let status = parse_task_status(status_str).ok_or_else(|| anyhow::anyhow!("invalid status '{status_str}'"))?;
    store.update_task_status(task_id, status)?;
    Ok(format!("Task {task_id} moved to {status_str}"))
}

fn tool_get_task_activity(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let task_id = args.get("task_id").and_then(|v| v.as_i64()).ok_or_else(|| anyhow::anyhow!("missing required 'task_id'"))?;
    let task = store.get_task(task_id)?.ok_or_else(|| anyhow::anyhow!("task {task_id} not found"))?;
    let Some(session_id) = &task.session_id else {
        return Ok(format!("Task {task_id} ({}) has no session_id — nothing to correlate.", task.title));
    };
    let events = store.session_events(&repo, session_id)?;
    if events.is_empty() {
        return Ok(format!("Task {task_id} ({}) has session_id '{session_id}' but no activity recorded under it yet.", task.title));
    }
    Ok(events.iter().map(|e| format!("- [{}] {}: {}", e.created_at, e.tool_name, e.description)).collect::<Vec<_>>().join("\n"))
}

fn tool_related_context(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let symbol_id = args.get("symbol_id").and_then(|v| v.as_i64()).ok_or_else(|| anyhow::anyhow!("missing required 'symbol_id'"))?;
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

    let results = agentops_retrieval::pattern_complete(store.as_ref(), &agentops_embeddings::LocalEmbedder, &repo, symbol_id, top_k)?;
    if results.is_empty() {
        return Ok("No related symbols found.".to_string());
    }
    if let Some(path_str) = get_str(args, "path") {
        maybe_record_session_event(Path::new(path_str), args, "related_context", &format!("found {} related item(s)", results.len()), Some(symbol_id), "hit")?;
    }
    Ok(results
        .iter()
        .map(|m| {
            let via = match m.via {
                agentops_retrieval::PatternCompletionSource::Similar(s) => format!("similar, {s:.2} cosine similarity"),
                agentops_retrieval::PatternCompletionSource::Graph(s) => format!("graph-connected, {s:.4} PageRank mass"),
            };
            let notes = if m.notes.is_empty() {
                String::new()
            } else {
                let rendered = m.notes.iter().map(|(kind, title, text, _, _)| format!("    - [{kind:?}] {title}: {text}")).collect::<Vec<_>>().join("\n");
                format!("\n{rendered}")
            };
            format!("- {} ({via}){}{notes}", m.node.name.as_deref().unwrap_or("(untitled)"), m.node.path.as_deref().map(|p| format!(" — {p}")).unwrap_or_default())
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn tool_explain_symbol(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let symbol_id = args.get("symbol_id").and_then(|v| v.as_i64()).ok_or_else(|| anyhow::anyhow!("missing required 'symbol_id'"))?;
    let config = agentops_llm::AnthropicConfig::from_env()?;
    let definition_id = agentops_llm::explain_symbol(store.as_ref(), &config, &repo, symbol_id)?;
    let definition = store.get_node(&repo, definition_id)?.ok_or_else(|| anyhow::anyhow!("definition node not found after creation"))?;
    if let Some(session_id) = get_str(args, "session_id") {
        store.record_session_event(&repo, session_id, "explain_symbol", &format!("explained symbol {symbol_id}"), None, "activity")?;
    }
    Ok(definition.content.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closes the exact gap `main` left open: every tool `list_tools()`
    /// advertises for a mode must actually be dispatchable in that mode,
    /// and a Full-only tool must never be listed OR allowed under Advisor.
    #[test]
    fn every_advisor_tool_is_dispatchable_and_no_full_tool_leaks_into_advisor() {
        let advisor_tools = list_tools(AccessMode::Advisor);
        let full_tools = list_tools(AccessMode::Full);
        assert!(full_tools.len() > advisor_tools.len(), "Full must see strictly more tools than Advisor");

        let advisor_names: std::collections::HashSet<_> = advisor_tools.iter().map(|t| t.name).collect();
        for spec in tool_specs() {
            if spec.access == AccessMode::Full {
                assert!(!advisor_names.contains(spec.name), "'{}' is Full-only and must not appear in Advisor's list_tools", spec.name);
                let result = call_tool(AccessMode::Advisor, spec.name, &json!({}));
                let call_result = result.expect("a known tool name must dispatch, not return 'unknown tool'");
                assert!(call_result.is_error, "'{}' must be refused when called directly under Advisor mode", spec.name);
            }
        }
    }

    /// Locks in the "knowledge-growing tools bypass the access-mode gate"
    /// decision: `add_note`/`ingest_notes` must be both listed and
    /// dispatchable under Advisor (read-only) mode, unlike every other
    /// write tool -- an org that hasn't opted a caller into `Full` access
    /// must still be able to grow the knowledge base.
    #[test]
    fn add_note_and_ingest_notes_are_available_under_advisor_mode() {
        let advisor_names: std::collections::HashSet<_> = list_tools(AccessMode::Advisor).iter().map(|t| t.name).collect();
        assert!(advisor_names.contains("add_note"), "{advisor_names:?}");
        assert!(advisor_names.contains("ingest_notes"), "{advisor_names:?}");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let result = call_tool(AccessMode::Advisor, "add_note", &json!({ "path": path, "title": "Advisor can still add notes", "body": "growing the knowledge base is not a Full-only action." })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
    }

    #[test]
    fn status_surfaces_top_gotchas_after_a_scan_with_notes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Token bug", "body": "verify_token has a known workaround for a bug." })).unwrap();
        // add_note alone doesn't refresh repo_state — only persist() does
        // (Module C's hook point) — so scan again to pick it up.
        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();

        let result = call_tool(AccessMode::Full, "status", &json!({ "path": path })).unwrap();
        assert!(result.content[0].text.contains("top gotchas"), "{:?}", result.content);
        assert!(result.content[0].text.contains("Token bug"), "{:?}", result.content);
    }

    #[test]
    fn status_omits_repo_state_section_when_none_recorded_yet() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        let result = call_tool(AccessMode::Full, "status", &json!({ "path": path })).unwrap();
        assert!(!result.content[0].text.contains("top gotchas"), "{:?}", result.content);
    }

    #[test]
    fn unknown_tool_name_is_rejected_before_dispatch() {
        let err = call_tool(AccessMode::Full, "totally_made_up_tool", &json!({})).unwrap_err();
        assert!(err.contains("unknown tool"));
    }

    /// Now that MCP registration is global (no per-repo `connect` step
    /// marks a repo as "known" anymore -- see the `agentops-cli` session
    /// that moved MCP config to user-level), a never-scanned repo and a
    /// genuinely-empty one look identical to `open_store`. `list_gotchas`
    /// must distinguish them so an agent gets nudged toward `scan_repo`
    /// instead of concluding there's just nothing to know yet.
    #[test]
    fn list_gotchas_on_a_never_scanned_repo_hints_at_scan_repo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let result = call_tool(AccessMode::Full, "list_gotchas", &json!({ "path": path })).unwrap();
        assert!(result.content[0].text.contains("never been scanned"), "{:?}", result.content);
        assert!(result.content[0].text.contains("scan_repo"), "{:?}", result.content);
    }

    #[test]
    fn list_gotchas_on_a_scanned_but_genuinely_empty_repo_does_not_suggest_scanning() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        let result = call_tool(AccessMode::Full, "list_gotchas", &json!({ "path": path })).unwrap();
        assert!(!result.content[0].text.contains("never been scanned"), "{:?}", result.content);
    }

    #[test]
    fn get_symbol_on_a_never_scanned_repo_hints_at_scan_repo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let result = call_tool(AccessMode::Full, "get_symbol", &json!({ "path": path, "name": "greet" })).unwrap();
        assert!(result.is_error);
        assert!(result.content[0].text.contains("never been scanned"), "{:?}", result.content);
    }

    #[test]
    fn scan_then_status_then_list_gotchas_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let scan_result = call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        assert!(!scan_result.is_error, "{:?}", scan_result.content);

        let status_result = call_tool(AccessMode::Full, "status", &json!({ "path": path })).unwrap();
        assert!(!status_result.is_error);
        assert!(status_result.content[0].text.contains("files: +1"));
    }

    #[test]
    fn add_note_writes_a_file_and_ingests_it_into_the_graph() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        let result = call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Token bug", "body": "verify_token has a known workaround for a bug." })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("gotcha"), "gotcha-shaped body must be auto-classified: {:?}", result.content);

        let gotchas_result = call_tool(AccessMode::Full, "list_gotchas", &json!({ "path": path })).unwrap();
        assert!(gotchas_result.content[0].text.contains("Token bug"));
    }

    /// Codebrain-2's actual payoff: a gotcha recorded against a symbol must
    /// resurface when that exact symbol is pulled via `get_symbol` — not
    /// just in the separately-generated full repo-map doc. Caught a real
    /// gap this way: `get_symbol` originally never rendered `Affects` edges
    /// at all, and separately, notes written before a repo's first scan had
    /// nothing to match against and stayed permanently unattached.
    #[test]
    fn get_symbol_surfaces_gotchas_affecting_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Token bug", "body": "verify_token has a known workaround for a bug." })).unwrap();

        let result = call_tool(AccessMode::Full, "get_symbol", &json!({ "path": path, "name": "verify_token" })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("Known gotchas affecting this symbol"), "{:?}", result.content);
        assert!(result.content[0].text.contains("Token bug"), "{:?}", result.content);
        assert!(!result.content[0].text.contains("possibly stale"), "a gotcha whose symbol hasn't changed since must not be flagged: {:?}", result.content);
    }

    /// Phase 2's actual payoff, not just a table nobody consults: a gotcha
    /// must be visibly flagged once the symbol it's attached to changes
    /// *after* the gotcha was last confirmed relevant — otherwise `get_symbol`
    /// keeps confidently repeating advice that may no longer be true.
    #[test]
    fn get_symbol_flags_a_gotcha_as_stale_once_the_symbol_changes_after_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Token bug", "body": "verify_token has a known workaround for a bug." })).unwrap();

        // Ensure the edit lands in a later second than the edge's
        // `updated_at` — same reasoning as agentops-graph's own
        // bi-temporal tests (CURRENT_TIMESTAMP has second granularity).
        std::thread::sleep(std::time::Duration::from_secs(1));
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    return True\n").unwrap();
        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();

        let result = call_tool(AccessMode::Full, "get_symbol", &json!({ "path": path, "name": "verify_token" })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("Token bug"), "{:?}", result.content);
        assert!(result.content[0].text.contains("possibly stale"), "a gotcha whose symbol changed since must be flagged: {:?}", result.content);
    }

    #[test]
    fn get_symbol_omits_the_gotchas_section_when_none_apply() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def untouched():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        let result = call_tool(AccessMode::Full, "get_symbol", &json!({ "path": path, "name": "untouched" })).unwrap();
        assert!(!result.content[0].text.contains("Known gotchas affecting this symbol"), "{:?}", result.content);
    }

    /// A repeatedly-reinforced gotcha must outrank a once-matched one when
    /// both affect the same symbol — re-adding the same note (same title,
    /// same body) re-matches and reinforces its existing edge via
    /// `connect_many`, rather than creating a duplicate.
    #[test]
    fn get_symbol_ranks_reinforced_gotchas_before_once_matched_ones() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Rare issue", "body": "verify_token has a rare bug." })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Reinforced issue", "body": "verify_token has a frequently-hit bug." })).unwrap();
        // Re-adding the same title/body re-matches and reinforces its
        // existing edge instead of duplicating it.
        for _ in 0..3 {
            call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Reinforced issue", "body": "verify_token has a frequently-hit bug." })).unwrap();
        }

        let result = call_tool(AccessMode::Full, "get_symbol", &json!({ "path": path, "name": "verify_token" })).unwrap();
        let text = &result.content[0].text;
        let reinforced_pos = text.find("Reinforced issue").expect("must appear");
        let rare_pos = text.find("Rare issue").expect("must appear");
        assert!(reinforced_pos < rare_pos, "the more-reinforced gotcha must be listed first: {text}");
    }

    #[test]
    fn list_gotchas_annotates_a_reduced_prominence_entry_and_sorts_it_last() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Full prominence gotcha", "body": "greet has a known bug, kept as-is." })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Niche gotcha", "body": "greet has a rare bug that only affects old Linux envs." })).unwrap();

        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());
        let niche = store.nodes_by_kind(&repo, NodeKind::Gotcha).unwrap().into_iter().find(|n| n.name.as_deref() == Some("Niche gotcha")).unwrap();
        store.set_curation(&repo, niche.id, agentops_graph::NodeProminence::Reduced, Some("only affects old Linux envs")).unwrap();

        let result = call_tool(AccessMode::Full, "list_gotchas", &json!({ "path": path })).unwrap();
        let text = &result.content[0].text;
        assert!(text.contains("⚠ reduced prominence — only affects old Linux envs"), "{text}");
        let full_pos = text.find("Full prominence gotcha").unwrap();
        let niche_pos = text.find("Niche gotcha").unwrap();
        assert!(full_pos < niche_pos, "a Reduced-prominence gotcha must sort after Full ones, not before: {text}");
    }

    #[test]
    fn get_symbol_annotates_and_demotes_a_reduced_prominence_gotcha() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Popular issue", "body": "verify_token has a common bug." })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Niche issue", "body": "verify_token has a rare bug." })).unwrap();

        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());
        let niche = store.nodes_by_kind(&repo, NodeKind::Gotcha).unwrap().into_iter().find(|n| n.name.as_deref() == Some("Niche issue")).unwrap();
        store.set_curation(&repo, niche.id, agentops_graph::NodeProminence::Reduced, Some("edge case, rarely hit")).unwrap();

        let result = call_tool(AccessMode::Full, "get_symbol", &json!({ "path": path, "name": "verify_token" })).unwrap();
        let text = &result.content[0].text;
        assert!(text.contains("⚠ reduced prominence — edge case, rarely hit"), "{text}");
        let popular_pos = text.find("Popular issue").unwrap();
        let niche_pos = text.find("Niche issue").unwrap();
        assert!(popular_pos < niche_pos, "even with identical raw weight, a Reduced gotcha must rank after a Full one: {text}");
    }

    #[test]
    fn semantic_search_dense_mode_annotates_a_reduced_prominence_hit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path, "with_embeddings": true })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Token gotcha", "body": "verify_token has a known workaround.", "with_embeddings": true })).unwrap();

        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());
        let gotcha = store.nodes_by_kind(&repo, NodeKind::Gotcha).unwrap().into_iter().next().unwrap();
        store.set_curation(&repo, gotcha.id, agentops_graph::NodeProminence::Reduced, Some("narrow edge case")).unwrap();

        let result = call_tool(AccessMode::Full, "local_semantic_search", &json!({ "path": path, "query": "verify_token workaround", "kind": "gotcha" })).unwrap();
        let text = &result.content[0].text;
        assert!(text.contains("⚠ reduced prominence — narrow edge case"), "{text}");
    }

    /// Module 6's actual payoff: two separate MCP calls (simulating two
    /// different clients/tools — e.g. Cursor scanning, Claude Code adding a
    /// note) that happen to share one `session_id` must show up as one
    /// correlated feed via `get_session`, protocol-native (nothing
    /// AgentOps-specific about how the caller got the id).
    #[test]
    fn two_calls_sharing_a_session_id_produce_one_correlated_feed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let session_id = "sess-abc123";

        let scan_result = call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path, "session_id": session_id })).unwrap();
        assert!(!scan_result.is_error, "{:?}", scan_result.content);
        let note_result = call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "found something", "body": "greet has a rare bug.", "session_id": session_id })).unwrap();
        assert!(!note_result.is_error, "{:?}", note_result.content);
        // A call under a *different* session must not leak into the feed.
        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path, "session_id": "sess-unrelated" })).unwrap();

        let session_result = call_tool(AccessMode::Full, "get_session", &json!({ "path": path, "session_id": session_id })).unwrap();
        assert!(!session_result.is_error, "{:?}", session_result.content);
        let text = &session_result.content[0].text;
        assert!(text.contains("scan_repo"), "{text}");
        assert!(text.contains("add_note"), "{text}");
        assert_eq!(text.matches("sess-unrelated").count(), 0, "the other session must not appear: {text}");
    }

    #[test]
    fn list_gotchas_and_get_symbol_record_hit_events_when_session_id_is_passed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let session_id = "sess-hits";

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Token bug", "body": "verify_token has a known workaround for a bug." })).unwrap();

        let gotchas_result = call_tool(AccessMode::Advisor, "list_gotchas", &json!({ "path": path, "session_id": session_id })).unwrap();
        assert!(!gotchas_result.is_error, "{:?}", gotchas_result.content);
        let symbol_result = call_tool(AccessMode::Advisor, "get_symbol", &json!({ "path": path, "name": "verify_token", "session_id": session_id })).unwrap();
        assert!(!symbol_result.is_error, "{:?}", symbol_result.content);

        let session_result = call_tool(AccessMode::Advisor, "get_session", &json!({ "path": path, "session_id": session_id })).unwrap();
        let text = &session_result.content[0].text;
        assert!(text.contains("list_gotchas"), "{text}");
        assert!(text.contains("get_symbol"), "{text}");
    }

    #[test]
    fn list_gotchas_does_not_record_a_hit_when_session_id_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Token bug", "body": "verify_token has a known workaround for a bug." })).unwrap();
        call_tool(AccessMode::Advisor, "list_gotchas", &json!({ "path": path })).unwrap();

        let session_result = call_tool(AccessMode::Advisor, "get_session", &json!({ "path": path, "session_id": "never-used" })).unwrap();
        assert!(session_result.content[0].text.contains("No activity recorded"), "{:?}", session_result.content);
    }

    /// Module 7's actual differentiator, not the CRUD: a task's own
    /// `session_id` transitively pulls in everything Module 6 already
    /// correlated under it — `get_task_activity` doesn't need its own
    /// separate write path.
    #[test]
    fn get_task_activity_surfaces_everything_correlated_under_the_tasks_session_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let session_id = "task-sess-1";

        let create_result = call_tool(AccessMode::Full, "create_task", &json!({ "path": path, "title": "fix greet", "session_id": session_id })).unwrap();
        assert!(!create_result.is_error, "{:?}", create_result.content);
        let task_id: i64 = create_result.content[0].text.split_whitespace().nth(2).unwrap().trim_end_matches(':').parse().unwrap();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path, "session_id": session_id })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "greet quirk", "body": "greet has a quirk.", "session_id": session_id })).unwrap();

        let activity = call_tool(AccessMode::Full, "get_task_activity", &json!({ "path": path, "task_id": task_id })).unwrap();
        assert!(!activity.is_error, "{:?}", activity.content);
        let text = &activity.content[0].text;
        assert!(text.contains("scan_repo"), "{text}");
        assert!(text.contains("add_note"), "{text}");
    }

    #[test]
    fn get_task_activity_reports_no_session_id_for_a_task_created_without_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let create_result = call_tool(AccessMode::Full, "create_task", &json!({ "path": path, "title": "untracked task" })).unwrap();
        let task_id: i64 = create_result.content[0].text.split_whitespace().nth(2).unwrap().trim_end_matches(':').parse().unwrap();

        let activity = call_tool(AccessMode::Full, "get_task_activity", &json!({ "path": path, "task_id": task_id })).unwrap();
        assert!(activity.content[0].text.contains("no session_id"), "{:?}", activity.content);
    }

    #[test]
    fn create_list_and_update_task_status_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let create_result = call_tool(AccessMode::Full, "create_task", &json!({ "path": path, "title": "do the thing" })).unwrap();
        assert!(!create_result.is_error, "{:?}", create_result.content);
        let task_id: i64 = create_result.content[0].text.split_whitespace().nth(2).unwrap().trim_end_matches(':').parse().unwrap();

        let list_result = call_tool(AccessMode::Full, "list_tasks", &json!({ "path": path })).unwrap();
        assert!(list_result.content[0].text.contains("do the thing"), "{:?}", list_result.content);
        assert!(list_result.content[0].text.contains("[todo]"), "{:?}", list_result.content);

        let update_result = call_tool(AccessMode::Full, "update_task_status", &json!({ "path": path, "task_id": task_id, "status": "in_progress" })).unwrap();
        assert!(!update_result.is_error, "{:?}", update_result.content);

        let list_after = call_tool(AccessMode::Full, "list_tasks", &json!({ "path": path })).unwrap();
        assert!(list_after.content[0].text.contains("[in_progress]"), "{:?}", list_after.content);
    }

    /// Phase 4's actual payoff, exposed through the existing tool rather
    /// than a new one: a literal function-name query surfaces the symbol
    /// even though it was never embedded (with_embeddings left off), via
    /// mode=hybrid's exact-match signal.
    #[test]
    fn semantic_search_hybrid_mode_finds_an_unembedded_symbol_by_exact_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token_signature():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path, "with_embeddings": false })).unwrap();

        let dense_result = call_tool(AccessMode::Advisor, "local_semantic_search", &json!({ "path": path, "query": "verify_token_signature" })).unwrap();
        assert!(dense_result.content[0].text.contains("No matches"), "dense-only search finds nothing since nothing was embedded: {:?}", dense_result.content);

        let hybrid_result = call_tool(AccessMode::Advisor, "local_semantic_search", &json!({ "path": path, "query": "verify_token_signature", "mode": "hybrid" })).unwrap();
        assert!(!hybrid_result.is_error, "{:?}", hybrid_result.content);
        assert!(hybrid_result.content[0].text.contains("verify_token_signature"), "{:?}", hybrid_result.content);
        assert!(hybrid_result.content[0].text.contains("exact"), "the exact signal must be the one that found it: {:?}", hybrid_result.content);
    }

    /// `related_context` (Initiative 4) end-to-end over MCP: a symbol
    /// referencing another symbol elsewhere in the file must surface that
    /// connected symbol's own recorded gotcha, without any LLM call.
    #[test]
    fn related_context_surfaces_a_graph_connected_symbols_gotcha() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def helper():\n    pass\n\ndef seed():\n    return helper()\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        call_tool(AccessMode::Full, "add_note", &json!({ "path": path, "title": "Helper workaround", "body": "helper has a known workaround for a bug." })).unwrap();

        let store = crate::store::open_store(std::path::Path::new(&path)).unwrap();
        let repo = crate::scan::repo_name(std::path::Path::new(&path));
        let seed_id = store.find_node(&repo, agentops_graph::NodeKind::Symbol, Some("auth.py"), Some("seed"), None).unwrap().unwrap().id;
        drop(store);

        let result = call_tool(AccessMode::Advisor, "related_context", &json!({ "path": path, "symbol_id": seed_id })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("helper"), "{:?}", result.content);
        assert!(result.content[0].text.contains("Helper workaround"), "{:?}", result.content);
        assert!(result.content[0].text.contains("graph-connected"), "{:?}", result.content);
    }

    #[test]
    fn related_context_reports_no_matches_for_an_isolated_symbol() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def lonely():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();

        let store = crate::store::open_store(std::path::Path::new(&path)).unwrap();
        let repo = crate::scan::repo_name(std::path::Path::new(&path));
        let id = store.find_node(&repo, agentops_graph::NodeKind::Symbol, Some("main.py"), Some("lonely"), None).unwrap().unwrap().id;
        drop(store);

        let result = call_tool(AccessMode::Advisor, "related_context", &json!({ "path": path, "symbol_id": id })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("No related symbols found"), "{:?}", result.content);
    }

    /// `end_session` (Initiative 5) refuses to consolidate for a
    /// `session_id` that never did anything in this repo -- the gate
    /// `run_embedding_consolidation` is deliberately never even reached
    /// for.
    #[test]
    fn end_session_reports_nothing_to_consolidate_for_an_unused_session_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();

        let result = call_tool(AccessMode::Full, "end_session", &json!({ "path": path, "session_id": "never-used" })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("nothing to consolidate"), "{:?}", result.content);
    }

    /// End-to-end over MCP: a session that did real work (scan_repo with a
    /// session_id) is a valid `end_session` call -- it must not error even
    /// though a single freshly-scanned tiny repo has far fewer than
    /// `MIN_REPLAY_PAIRS` plasticity-bearing edges yet, so the honest
    /// outcome here is a graceful skip, not a promoted model. The heavier
    /// "enough real signal to actually train and promote" path is already
    /// covered by `agentops-embeddings-train`'s own
    /// `consolidate_trains_and_promotes_on_a_first_real_run` test; this
    /// test's job is only the MCP tool wiring (session gating, dispatch,
    /// response formatting), not re-proving the ML internals.
    #[test]
    fn end_session_runs_consolidation_when_the_session_did_real_work() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path, "session_id": "sess-e2e" })).unwrap();

        let result = call_tool(AccessMode::Full, "end_session", &json!({ "path": path, "session_id": "sess-e2e" })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("Consolidation skipped:"), "a tiny fresh repo has too few plasticity-bearing pairs to train on yet: {:?}", result.content);

        let repo = crate::scan::repo_name(dir.path());
        if let Some(home) = dirs::home_dir() {
            let _ = std::fs::remove_dir_all(home.join(".agentops").join("models").join(&repo));
        }
    }

    /// `mode=gist_then_detail` (Initiative 2) end-to-end over MCP: a scan
    /// with embeddings enabled indexes the generated overview DocSection as
    /// its own searchable node, and a query matching it must return real
    /// results, not "No matches" -- the whole point of Initiative 2 being
    /// that docgen's output stops being search-invisible.
    #[test]
    fn semantic_search_gist_then_detail_mode_returns_real_results_after_a_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path, "with_embeddings": true })).unwrap();

        let result = call_tool(AccessMode::Advisor, "local_semantic_search", &json!({ "path": path, "query": "repository overview symbols indexed", "mode": "gist_then_detail" })).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(!result.content[0].text.contains("No matches"), "the indexed overview section must be findable: {:?}", result.content);
    }

    /// `graph_expand` (Initiative 0) is opt-in over the MCP surface:
    /// omitting it must leave the ranking/fused score untouched, and passing
    /// it must not error even for a lone symbol with no `Affects`/
    /// `References` edges -- it trivially becomes its own PPR seed (nonzero
    /// self-restart mass), so the *text* does gain a `graph` annotation, but
    /// the underlying `fused_score` and hit identity/order must be identical
    /// either way, since there's no second node for activation to actually
    /// redistribute toward.
    #[test]
    fn semantic_search_hybrid_mode_accepts_graph_expand_without_erroring_or_changing_the_fused_score() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token_signature():\n    pass\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();
        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path, "with_embeddings": false })).unwrap();

        let default_result = call_tool(AccessMode::Advisor, "local_semantic_search", &json!({ "path": path, "query": "verify_token_signature", "mode": "hybrid" })).unwrap();
        let expanded_result = call_tool(AccessMode::Advisor, "local_semantic_search", &json!({ "path": path, "query": "verify_token_signature", "mode": "hybrid", "graph_expand": true })).unwrap();
        assert!(!expanded_result.is_error, "{:?}", expanded_result.content);
        assert!(expanded_result.content[0].text.contains("verify_token_signature"), "{:?}", expanded_result.content);
        assert!(expanded_result.content[0].text.contains("score 0.0328"), "the fused score itself must be unaffected by graph_expand: {:?}", expanded_result.content);
        assert!(!default_result.content[0].text.contains("graph "), "graph_expand off must never render a graph-score annotation: {:?}", default_result.content);
        assert!(expanded_result.content[0].text.contains("graph "), "graph_expand on must render the annotation once a node has nonzero PPR mass: {:?}", expanded_result.content);
    }

    #[test]
    fn get_session_reports_no_activity_for_a_session_id_never_used() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        call_tool(AccessMode::Full, "scan_repo", &json!({ "path": path })).unwrap();
        let result = call_tool(AccessMode::Full, "get_session", &json!({ "path": path, "session_id": "never-used" })).unwrap();
        assert!(!result.is_error);
        assert!(result.content[0].text.contains("No activity recorded"), "{:?}", result.content);
    }
}
