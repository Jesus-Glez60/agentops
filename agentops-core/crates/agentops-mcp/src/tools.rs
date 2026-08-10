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

use agentops_graph::{GraphStore, NodeKind, SqliteGraphStore};
use serde_json::{json, Value};

use crate::protocol::{CallToolResult, ToolAnnotations, ToolDefinition};
use crate::scan::{graph_db_path, repo_name};

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
            description: "Lists every Gotcha node recorded for a repo.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            handler: tool_list_gotchas,
        },
        ToolSpec {
            name: "get_symbol",
            description: "Looks up a symbol by name (optionally disambiguated by file path).",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || {
                json!({ "type": "object", "properties": { "path": { "type": "string" }, "name": { "type": "string" }, "file": { "type": "string" } }, "required": ["path", "name"] })
            },
            handler: tool_get_symbol,
        },
        ToolSpec {
            name: "get_changelog",
            description: "Lists recent scans for a repo, most recent first.",
            access: AccessMode::Advisor,
            annotations: READ_ONLY,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "limit": { "type": "integer" } }, "required": ["path"] }),
            handler: tool_get_changelog,
        },
        ToolSpec {
            name: "scan_repo",
            description: "Scans the repo at `path` and persists it to the graph store — token-bounded change detection (Added/Changed/Removed), safe to call repeatedly.",
            access: AccessMode::Full,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            handler: tool_scan_repo,
        },
        ToolSpec {
            name: "add_note",
            description: "Writes a new note (gotcha/decision/knowledge) to the repo's notes folder and ingests it into the graph in one step — the write-back tool for an agent that just learned something worth remembering. Omit note_type to let it be classified automatically.",
            access: AccessMode::Full,
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
                    },
                    "required": ["path", "title", "body"],
                })
            },
            handler: tool_add_note,
        },
        ToolSpec {
            name: "ingest_notes",
            description: "Walks a notes folder (a real vault or an unorganized one) and ingests every note into the graph — classifying freeform notes with no frontmatter/folder signal via the heuristic classifier.",
            access: AccessMode::Full,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "notes_path": { "type": "string" } }, "required": ["path"] }),
            handler: tool_ingest_notes,
        },
        ToolSpec {
            name: "explain_symbol",
            description: "Explains a symbol via the Anthropic API and persists the result as a Definition node linked to it. Requires AGENTOPS_ANTHROPIC_API_KEY. Costs a real API call — never run automatically during a scan.",
            access: AccessMode::Full,
            annotations: ToolAnnotations { read_only_hint: false, destructive_hint: false, idempotent_hint: false, open_world_hint: true },
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "symbol_id": { "type": "integer" } }, "required": ["path", "symbol_id"] }),
            handler: tool_explain_symbol,
        },
        ToolSpec {
            name: "init_agents_md",
            description: "Writes (or refreshes) AGENTS.md for this repo with a resolved NOTES_PATH, and ensures .gitignore excludes generated scan output. Lets an agent bootstrap the write-back protocol for itself over MCP, without needing shell access to the CLI's `install` command.",
            access: AccessMode::Full,
            annotations: WRITE_IDEMPOTENT,
            input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "notes_path": { "type": "string" } }, "required": ["path"] }),
            handler: tool_init_agents_md,
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
        Err(e) => CallToolResult::error(e.to_string()),
    })
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn repo_context(args: &Value) -> anyhow::Result<(SqliteGraphStore, String)> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let path = Path::new(path_str);
    let store = SqliteGraphStore::open(&graph_db_path(path))?;
    Ok((store, repo_name(path)))
}

fn tool_status(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    match store.latest_scan(&repo)? {
        Some(scan) => Ok(format!(
            "repo: {repo}\nlast scan: {}\nfiles: +{} ~{} -{}\nsymbols: +{} ~{} -{}",
            scan.started_at, scan.files_added, scan.files_changed, scan.files_removed, scan.symbols_added, scan.symbols_changed, scan.symbols_removed
        )),
        None => Ok(format!("repo: {repo}\nno scans recorded yet — call scan_repo first")),
    }
}

fn tool_list_gotchas(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let gotchas = store.nodes_by_kind(&repo, NodeKind::Gotcha)?;
    if gotchas.is_empty() {
        return Ok("No gotchas recorded.".to_string());
    }
    Ok(gotchas.iter().map(|n| format!("- {} (node {})", n.name.as_deref().unwrap_or("(untitled)"), n.id)).collect::<Vec<_>>().join("\n"))
}

fn tool_get_symbol(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let name = get_str(args, "name").ok_or_else(|| anyhow::anyhow!("missing required 'name'"))?;
    let file = get_str(args, "file").map(Path::new);
    let id = agentops_llm::find_symbol_by_name(&store, &repo, name, file)?;
    let node = store.get_node(&repo, id)?.ok_or_else(|| anyhow::anyhow!("symbol resolved but node #{id} not found"))?;
    Ok(format!(
        "{} ({}) — {}:{}-{}\n\n{}",
        node.name.as_deref().unwrap_or(name),
        "symbol",
        node.path.as_deref().unwrap_or("?"),
        node.start_line.unwrap_or(0),
        node.end_line.unwrap_or(0),
        node.content.as_deref().unwrap_or("")
    ))
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
    let summary = crate::scan::scan_and_persist(Path::new(path_str))?;
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

    let result = crate::notes::add_note(Path::new(path_str), title, body, note_type, &tags, explicit_notes_path.as_deref())?;
    let type_str = match result.note_type {
        agentops_notes::NoteType::Gotcha => "gotcha",
        agentops_notes::NoteType::Decision => "decision",
        agentops_notes::NoteType::Knowledge => "knowledge",
        agentops_notes::NoteType::Context => "context",
    };
    Ok(format!("Wrote {} ({type_str}) and ingested it ({} edge(s) to related symbols).", result.file_path.display(), result.edges_written))
}

fn tool_ingest_notes(args: &Value) -> anyhow::Result<String> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let explicit_notes_path = get_str(args, "notes_path").map(PathBuf::from);

    let classifier = agentops_notes::HeuristicClassifier;
    let matcher = agentops_notes::WordBoundaryMatcher::default();
    let summary = crate::notes::ingest_notes_dir(Path::new(path_str), explicit_notes_path.as_deref(), &classifier, &matcher)?;

    let notes_dir = agentops_notes::resolve_notes_path(Path::new(path_str), explicit_notes_path.as_deref());
    Ok(format!("Ingested {} note(s) from {}, wrote {} edge(s).", summary.notes_written, notes_dir.display(), summary.edges_written))
}

fn tool_init_agents_md(args: &Value) -> anyhow::Result<String> {
    let path_str = get_str(args, "path").ok_or_else(|| anyhow::anyhow!("missing required 'path'"))?;
    let notes_path = get_str(args, "notes_path").map(PathBuf::from);
    let result = crate::init::init_agents_md(Path::new(path_str), notes_path.as_deref())?;
    Ok(format!("Wrote {} (NOTES_PATH: {})", result.agents_md_path.display(), result.notes_path.display()))
}

fn tool_explain_symbol(args: &Value) -> anyhow::Result<String> {
    let (store, repo) = repo_context(args)?;
    let symbol_id = args.get("symbol_id").and_then(|v| v.as_i64()).ok_or_else(|| anyhow::anyhow!("missing required 'symbol_id'"))?;
    let config = agentops_llm::AnthropicConfig::from_env()?;
    let definition_id = agentops_llm::explain_symbol(&store, &config, &repo, symbol_id)?;
    let definition = store.get_node(&repo, definition_id)?.ok_or_else(|| anyhow::anyhow!("definition node not found after creation"))?;
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

    #[test]
    fn unknown_tool_name_is_rejected_before_dispatch() {
        let err = call_tool(AccessMode::Full, "totally_made_up_tool", &json!({})).unwrap_err();
        assert!(err.contains("unknown tool"));
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
}
