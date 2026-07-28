//! Tool definitions and dispatch — this is where `AccessMode` enforcement
//! actually lives. `Advisor` mode's tool list simply never includes the
//! write-capable tools; it's not that the model is told not to call them,
//! they don't exist for it to call. `call_tool` re-checks the mode defensively
//! anyway (belt-and-suspenders — see the plan's §Security on structural vs.
//! prompted boundaries).

use std::path::{Path, PathBuf};

use agentops_graph::{GraphStore, NewNode, NodeKind, SqliteGraphStore};
use agentops_notes::AffectsTarget;
use serde_json::{json, Value};

use crate::protocol::{CallToolResult, ToolDefinition};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessMode {
    /// Plans/reviews only — write-capable tools are never registered.
    Advisor,
    /// Full agent access, including scanning and note-taking.
    Full,
}

fn graph_db_path(repo: &Path) -> PathBuf {
    repo.join(".context").join("graph.db")
}

/// Read-only tools — available in both `Advisor` and `Full` mode.
const READ_ONLY_TOOLS: &[&str] = &["status", "list_gotchas", "repo_map"];

/// Write-capable tools — available only in `Full` mode. Every one of these
/// writes to disk (the graph store, and/or generated files) or otherwise
/// changes state; none of them modify the user's actual source code, but they
/// still gate on `AccessMode` since they're the closest thing this server has
/// to a "write" capability.
const WRITE_TOOLS: &[&str] = &["scan_repo", "add_note", "generate_docs"];

/// Returns the tool definitions visible for `mode` — this list is what a
/// client sees from `tools/list`; write tools are structurally absent in
/// `Advisor` mode, not merely discouraged.
pub fn list_tools(mode: AccessMode) -> Vec<ToolDefinition> {
    let mut tools = vec![
        ToolDefinition {
            name: "status",
            description: "Graph stats (files/symbols/gotchas/decisions) for an already-scanned repo.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        },
        ToolDefinition {
            name: "list_gotchas",
            description: "List recorded gotcha/decision notes and the symbols they affect.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        },
        ToolDefinition {
            name: "repo_map",
            description: "Render the ranked onboarding doc (repo-map.md content) from the existing graph, without writing it to disk.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        },
    ];

    if mode == AccessMode::Full {
        tools.push(ToolDefinition {
            name: "scan_repo",
            description: "Scan a repo into the neuron graph and write AGENTS.md. Writes to disk.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        });
        tools.push(ToolDefinition {
            name: "add_note",
            description: "Record a gotcha or decision note, optionally edge-connected to a symbol by name. Writes to disk.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "kind": { "type": "string", "enum": ["gotcha", "decision"] },
                    "title": { "type": "string" },
                    "text": { "type": "string" },
                    "affects": { "type": "string", "description": "Symbol name this note affects, if any." },
                },
                "required": ["path", "kind", "title", "text"],
            }),
        });
        tools.push(ToolDefinition {
            name: "generate_docs",
            description: "Render and write repo-map.md from the existing graph. Writes to disk.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        });
    }

    tools
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// Dispatches a `tools/call` for `name`. Returns `Err` (a JSON-RPC-level
/// error, not a tool-result error) if `name` isn't a tool this mode is allowed
/// to call at all — this is the defensive re-check; the primary enforcement
/// is `list_tools` never advertising the tool in the first place.
pub fn call_tool(mode: AccessMode, name: &str, args: &Value) -> Result<CallToolResult, String> {
    let allowed = READ_ONLY_TOOLS.contains(&name) || (mode == AccessMode::Full && WRITE_TOOLS.contains(&name));
    if !allowed {
        return Err(format!(
            "tool '{name}' is not available in {mode:?} mode{}",
            if WRITE_TOOLS.contains(&name) { " (write-capable tools require Full access mode)" } else { "" }
        ));
    }

    let result = match name {
        "status" => tool_status(args),
        "list_gotchas" => tool_list_gotchas(args),
        "repo_map" => tool_repo_map(args),
        "scan_repo" => tool_scan_repo(args),
        "add_note" => tool_add_note(args),
        "generate_docs" => tool_generate_docs(args),
        _ => return Err(format!("unknown tool '{name}'")),
    };

    Ok(match result {
        Ok(text) => CallToolResult::success(text),
        Err(e) => CallToolResult::error(e.to_string()),
    })
}

fn require_path(args: &Value) -> anyhow::Result<PathBuf> {
    get_str(args, "path").map(PathBuf::from).ok_or_else(|| anyhow::anyhow!("missing required 'path' argument"))
}

fn tool_status(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    Ok(format!(
        "files: {}\nsymbols: {}\ngotchas: {}\ndecisions: {}",
        store.nodes_by_kind(NodeKind::File)?.len(),
        store.nodes_by_kind(NodeKind::Symbol)?.len(),
        store.nodes_by_kind(NodeKind::Gotcha)?.len(),
        store.nodes_by_kind(NodeKind::Decision)?.len(),
    ))
}

fn tool_list_gotchas(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let gotchas = store.nodes_by_kind(NodeKind::Gotcha)?;
    if gotchas.is_empty() {
        return Ok("No gotchas recorded.".to_string());
    }

    let mut out = String::new();
    for g in gotchas {
        out.push_str(&format!("- {}: {}\n", g.name.as_deref().unwrap_or("(untitled)"), g.content.as_deref().unwrap_or("")));
        for edge in store.edges_from(g.id)? {
            if let Some(target) = store.get_node(edge.dst_id)? {
                out.push_str(&format!("  affects: {}\n", target.name.as_deref().unwrap_or("<unknown>")));
            }
        }
    }
    Ok(out)
}

fn tool_repo_map(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let report = agentops_scanner::scan_repo(&path)?;
    let ranked: Vec<PathBuf> = agentops_scanner::rank_files(&report.files).into_iter().map(|(p, _)| p).collect();
    let store = SqliteGraphStore::open(&db_path)?;
    let repo_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    agentops_docgen::render_onboarding_doc(&store, repo_name, &ranked)
}

fn tool_scan_repo(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let repo_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_string();

    let report = agentops_scanner::scan_repo(&path)?;
    let db_path = graph_db_path(&path);
    let store = SqliteGraphStore::open(&db_path)?;

    let mut symbol_count = 0;
    for file in &report.files {
        let path_str = file.path.to_string_lossy().to_string();
        store.add_node(NewNode {
            kind: NodeKind::File,
            repo: repo_name.clone(),
            path: Some(path_str.clone()),
            name: None,
            start_line: None,
            end_line: None,
            content: None,
        })?;
        for symbol in &file.symbols {
            store.add_node(NewNode {
                kind: NodeKind::Symbol,
                repo: repo_name.clone(),
                path: Some(path_str.clone()),
                name: Some(symbol.name.clone()),
                start_line: Some(symbol.start_line as i64),
                end_line: Some(symbol.end_line as i64),
                content: Some(symbol.source.clone()),
            })?;
            symbol_count += 1;
        }
    }

    let opts = agentops_agents_md::GenerateOptions { claude_code_installed: false, repo_map_path: Some("repo-map.md".to_string()) };
    let agents_md = agentops_agents_md::generate(&path, &opts);
    std::fs::write(path.join("AGENTS.md"), &agents_md)?;

    Ok(format!(
        "Scanned {} files ({} symbols, {} secrets redacted). Wrote AGENTS.md and {}.",
        report.files.len(),
        symbol_count,
        report.redacted_count,
        db_path.display()
    ))
}

fn tool_add_note(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let kind = get_str(args, "kind").ok_or_else(|| anyhow::anyhow!("missing required 'kind' argument"))?;
    let title = get_str(args, "title").ok_or_else(|| anyhow::anyhow!("missing required 'title' argument"))?;
    let text = get_str(args, "text").ok_or_else(|| anyhow::anyhow!("missing required 'text' argument"))?;
    let affects = get_str(args, "affects");

    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let repo_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_string();
    let target = match affects {
        Some(name) => AffectsTarget::SymbolName(name),
        None => AffectsTarget::None,
    };

    let id = match kind {
        "gotcha" => agentops_notes::add_gotcha(&store, &repo_name, title, text, target)?,
        "decision" => agentops_notes::add_decision(&store, &repo_name, title, text, target)?,
        other => anyhow::bail!("invalid kind '{other}', expected 'gotcha' or 'decision'"),
    };

    Ok(format!("Recorded {kind} node #{id}."))
}

fn tool_generate_docs(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let content = tool_repo_map(args)?;
    let out_path = path.join("repo-map.md");
    agentops_docgen::write_to_file(&content, &out_path)?;
    Ok(format!("Wrote {}", out_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn advisor_mode_hides_write_tools() {
        let advisor_names: Vec<&str> = list_tools(AccessMode::Advisor).iter().map(|t| t.name).collect();
        assert!(advisor_names.contains(&"status"));
        assert!(!advisor_names.contains(&"scan_repo"));
        assert!(!advisor_names.contains(&"add_note"));
        assert!(!advisor_names.contains(&"generate_docs"));

        let full_names: Vec<&str> = list_tools(AccessMode::Full).iter().map(|t| t.name).collect();
        assert!(full_names.contains(&"scan_repo"));
        assert!(full_names.contains(&"add_note"));
        assert!(full_names.contains(&"generate_docs"));
    }

    #[test]
    fn advisor_mode_refuses_write_tool_calls_defensively() {
        let result = call_tool(AccessMode::Advisor, "scan_repo", &json!({"path": "/tmp/whatever"}));
        assert!(result.is_err(), "scan_repo must be refused even if somehow called in Advisor mode");
    }

    #[test]
    fn full_mode_scan_then_status_then_note_then_docgen_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.py"), "def verify_token(t):\n    return True\n").unwrap();

        let scan_result = call_tool(AccessMode::Full, "scan_repo", &json!({"path": root})).unwrap();
        assert!(!scan_result.is_error, "{:?}", scan_result.content);

        let status = call_tool(AccessMode::Full, "status", &json!({"path": root})).unwrap();
        assert!(status.content[0].text.contains("symbols: 1"));

        let note = call_tool(
            AccessMode::Full,
            "add_note",
            &json!({"path": root, "kind": "gotcha", "title": "t", "text": "naive check", "affects": "verify_token"}),
        )
        .unwrap();
        assert!(!note.is_error, "{:?}", note.content);

        let gotchas = call_tool(AccessMode::Full, "list_gotchas", &json!({"path": root})).unwrap();
        assert!(gotchas.content[0].text.contains("naive check"));
        assert!(gotchas.content[0].text.contains("verify_token"));

        let docs = call_tool(AccessMode::Full, "generate_docs", &json!({"path": root})).unwrap();
        assert!(!docs.is_error, "{:?}", docs.content);
        assert!(root.join("repo-map.md").exists());
    }

    #[test]
    fn unknown_tool_name_is_rejected() {
        let result = call_tool(AccessMode::Full, "delete_everything", &json!({}));
        assert!(result.is_err());
    }
}
