//! Tool definitions and dispatch — this is where `AccessMode` enforcement
//! actually lives. `Advisor` mode's tool list simply never includes the
//! write-capable tools; it's not that the model is told not to call them,
//! they don't exist for it to call. `call_tool` re-checks the mode defensively
//! anyway (belt-and-suspenders — see the plan's §Security on structural vs.
//! prompted boundaries).

use std::path::{Path, PathBuf};

use agentops_graph::{EdgeRelation, GraphStore, NodeKind, SqliteGraphStore};
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
const READ_ONLY_TOOLS: &[&str] = &["status", "list_gotchas", "repo_map", "get_dependencies", "get_symbol", "ast_search", "get_changelog"];

/// Write-capable tools — available only in `Full` mode. Every one of these
/// writes to disk (the graph store, and/or generated files) or otherwise
/// changes state; none of them modify the user's actual source code, but they
/// still gate on `AccessMode` since they're the closest thing this server has
/// to a "write" capability.
const WRITE_TOOLS: &[&str] = &["scan_repo", "add_note", "generate_docs", "explain_symbol", "ingest_notes"];

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
        ToolDefinition {
            name: "get_dependencies",
            description: "What a file depends on and what depends on it, from the persisted DependsOn graph (relative-import resolution only — external package imports aren't tracked as edges).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "file": { "type": "string", "description": "File path relative to the repo root, as it appears in repo_map/status output." },
                },
                "required": ["path", "file"],
            }),
        },
        ToolDefinition {
            name: "get_symbol",
            description: "Exact lookup of a symbol by name — its file, line range, and full source. Faster and cheaper than reading a whole file when you already know the name.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "name": { "type": "string" },
                },
                "required": ["path", "name"],
            }),
        },
        ToolDefinition {
            name: "ast_search",
            description: "Find symbols by a case-insensitive substring match on their name — useful when you don't know the exact name get_symbol would need. Returns each match's name, file, and line range (not full source — call get_symbol for that).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "query": { "type": "string" },
                },
                "required": ["path", "query"],
            }),
        },
        ToolDefinition {
            name: "get_changelog",
            description: "What changed in this repo's code/notes across scans. With no other args, shows the full added/changed/removed diff for the most recent scan. With since_scan_id, shows that specific scan's diff instead. With limit and neither of the above, lists that many recent scans' summaries (counts only, no per-item diff) — useful for seeing scan history over time before drilling into one.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "since_scan_id": { "type": "integer", "description": "Show this specific scan's diff instead of the most recent one." },
                    "limit": { "type": "integer", "description": "List this many recent scan summaries instead of one scan's full diff." },
                },
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
        tools.push(ToolDefinition {
            name: "ingest_notes",
            description: "Recursively ingest a markdown notes/vault folder, symbol-matching each note into this repo's graph (Gotcha/Decision/Note nodes, Affects-connected to matched symbols). dry_run previews the note -> symbol match table without writing. llm_match re-ranks each note's candidates with one Anthropic API call (requires AGENTOPS_ANTHROPIC_API_KEY) instead of trusting the word-boundary match as-is.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Repo to attach ingested notes to." },
                    "notes": { "type": "string", "description": "Directory to recursively walk for *.md notes." },
                    "dry_run": { "type": "boolean" },
                    "llm_match": { "type": "boolean" },
                    "min_name_len": { "type": "integer", "description": "Minimum symbol-name length to consider as a match candidate. Defaults to 4." },
                },
                "required": ["path", "notes"],
            }),
        });
        tools.push(ToolDefinition {
            name: "explain_symbol",
            description: "Generate an LLM explanation of what a symbol does (Anthropic API — requires AGENTOPS_ANTHROPIC_API_KEY) and record it as a Definition node connected to the symbol. On-demand only, never runs automatically during a scan. Prefer symbol_id when you already have one (e.g. from get_symbol/ast_search/get_changelog); symbol_name requires file to disambiguate if the name isn't unique in the repo.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "symbol_id": { "type": "integer" },
                    "symbol_name": { "type": "string" },
                    "file": { "type": "string", "description": "File path relative to the repo root, to disambiguate symbol_name if it isn't unique." },
                },
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
        "get_dependencies" => tool_get_dependencies(args),
        "get_symbol" => tool_get_symbol(args),
        "ast_search" => tool_ast_search(args),
        "get_changelog" => tool_get_changelog(args),
        "scan_repo" => tool_scan_repo(args),
        "add_note" => tool_add_note(args),
        "generate_docs" => tool_generate_docs(args),
        "explain_symbol" => tool_explain_symbol(args),
        "ingest_notes" => tool_ingest_notes(args),
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
        "files: {}\nsymbols: {}\ngotchas: {}\ndecisions: {}\nnotes: {}\ndefinitions: {}",
        store.nodes_by_kind(NodeKind::File)?.len(),
        store.nodes_by_kind(NodeKind::Symbol)?.len(),
        store.nodes_by_kind(NodeKind::Gotcha)?.len(),
        store.nodes_by_kind(NodeKind::Decision)?.len(),
        store.nodes_by_kind(NodeKind::Note)?.len(),
        store.nodes_by_kind(NodeKind::Definition)?.len(),
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

fn tool_get_dependencies(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let file = get_str(args, "file").ok_or_else(|| anyhow::anyhow!("missing required 'file' argument"))?;
    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;

    let files = store.nodes_by_kind(NodeKind::File)?;
    let Some(target) = files.iter().find(|f| f.path.as_deref() == Some(file)) else {
        anyhow::bail!("no file node found for {file:?} — check it matches repo_map/status output exactly");
    };

    let mut out = String::new();
    let depends_on: Vec<_> = store.edges_from(target.id)?.into_iter().filter(|e| e.relation == EdgeRelation::DependsOn).collect();
    out.push_str("Depends on:\n");
    if depends_on.is_empty() {
        out.push_str("  (none resolved — only relative-path imports are tracked as edges)\n");
    }
    for edge in depends_on {
        if let Some(dep) = store.get_node(edge.dst_id)? {
            out.push_str(&format!("  - {}\n", dep.path.as_deref().unwrap_or("<unknown>")));
        }
    }

    let depended_on_by: Vec<_> = store.edges_to(target.id)?.into_iter().filter(|e| e.relation == EdgeRelation::DependsOn).collect();
    out.push_str("Depended on by:\n");
    if depended_on_by.is_empty() {
        out.push_str("  (none)\n");
    }
    for edge in depended_on_by {
        if let Some(dep) = store.get_node(edge.src_id)? {
            out.push_str(&format!("  - {}\n", dep.path.as_deref().unwrap_or("<unknown>")));
        }
    }

    Ok(out)
}

fn tool_get_symbol(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let name = get_str(args, "name").ok_or_else(|| anyhow::anyhow!("missing required 'name' argument"))?;
    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;

    let matches: Vec<_> = store.nodes_by_kind(NodeKind::Symbol)?.into_iter().filter(|n| n.name.as_deref() == Some(name)).collect();
    if matches.is_empty() {
        anyhow::bail!("no symbol named {name:?} found — try ast_search if you're not sure of the exact name");
    }

    let mut out = String::new();
    for symbol in matches {
        out.push_str(&format!(
            "{} ({}:{}-{})\n\n{}\n\n",
            symbol.name.as_deref().unwrap_or("<unnamed>"),
            symbol.path.as_deref().unwrap_or("<unknown>"),
            symbol.start_line.unwrap_or(0),
            symbol.end_line.unwrap_or(0),
            symbol.content.as_deref().unwrap_or(""),
        ));
    }
    Ok(out)
}

fn tool_ast_search(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let query = get_str(args, "query").ok_or_else(|| anyhow::anyhow!("missing required 'query' argument"))?;
    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let query_lower = query.to_lowercase();

    let matches: Vec<_> =
        store.nodes_by_kind(NodeKind::Symbol)?.into_iter().filter(|n| n.name.as_deref().is_some_and(|name| name.to_lowercase().contains(&query_lower))).collect();

    if matches.is_empty() {
        return Ok(format!("No symbols matching {query:?}."));
    }

    let mut out = String::new();
    for symbol in matches {
        out.push_str(&format!(
            "{} — {}:{}-{}\n",
            symbol.name.as_deref().unwrap_or("<unnamed>"),
            symbol.path.as_deref().unwrap_or("<unknown>"),
            symbol.start_line.unwrap_or(0),
            symbol.end_line.unwrap_or(0),
        ));
    }
    Ok(out)
}

fn format_scan_summary(scan: &agentops_graph::ScanHistoryRow) -> String {
    format!(
        "Scan #{} ({} -> {}){}\nFiles: +{} ~{} -{}   Symbols: +{} ~{} -{}   Notes added: {}\n",
        scan.id,
        scan.started_at,
        scan.finished_at,
        scan.git_sha.as_deref().map(|s| format!(" @ {s}")).unwrap_or_default(),
        scan.files_added,
        scan.files_changed,
        scan.files_removed,
        scan.symbols_added,
        scan.symbols_changed,
        scan.symbols_removed,
        scan.notes_added,
    )
}

fn tool_get_changelog(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let repo_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_string();

    let since_scan_id = args.get("since_scan_id").and_then(|v| v.as_i64());
    let limit = args.get("limit").and_then(|v| v.as_i64());

    // `limit` with neither other arg: an overview of recent scans, not one
    // scan's full diff — useful before deciding which scan to drill into.
    if since_scan_id.is_none() {
        if let Some(limit) = limit {
            let scans = store.list_scans(&repo_name, limit)?;
            if scans.is_empty() {
                return Ok("No scans recorded yet for this repo.".to_string());
            }
            return Ok(scans.iter().map(format_scan_summary).collect::<Vec<_>>().join("\n"));
        }
    }

    let scan_id = match since_scan_id {
        Some(id) => id,
        None => match store.latest_scan(&repo_name)? {
            Some(latest) => latest.id,
            None => return Ok("No scans recorded yet for this repo.".to_string()),
        },
    };
    let Some(scan) = store.get_scan(scan_id)? else {
        anyhow::bail!("no scan #{scan_id} found for this repo");
    };
    let entries = store.scan_diff(scan_id)?;

    let mut out = format_scan_summary(&scan);
    if entries.is_empty() {
        out.push_str("(no changes)\n");
    }
    for e in &entries {
        // Symbol entries carry both `path` and `name` (a symbol always lives
        // in a file) -- show the name, since that's what's actually
        // added/changed/removed; file entries only ever carry `path`.
        let label = match e.name.as_deref() {
            Some(name) => match e.path.as_deref() {
                Some(path) => format!("{name} ({path})"),
                None => name.to_string(),
            },
            None => e.path.as_deref().unwrap_or("<unknown>").to_string(),
        };
        out.push_str(&format!("  {} {} {}\n", e.change, e.kind, label));
    }
    Ok(out)
}

fn tool_scan_repo(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let db_path = graph_db_path(&path);

    let summary = crate::scan::scan_and_persist(&path)?;

    let opts = agentops_agents_md::GenerateOptions { claude_code_installed: false, repo_map_path: Some("repo-map.md".to_string()) };
    let agents_md = agentops_agents_md::generate(&path, &opts);
    std::fs::write(path.join("AGENTS.md"), &agents_md)?;

    let mut msg = format!(
        "Scanned {} files ({} symbols, {} dependency edges). Wrote AGENTS.md and {}.",
        summary.files,
        summary.symbols,
        summary.dependency_edges,
        db_path.display()
    );
    if summary.pruned_files > 0 || summary.pruned_symbols > 0 {
        msg.push_str(&format!(" Pruned {} stale file(s) and {} stale symbol(s) from prior scans.", summary.pruned_files, summary.pruned_symbols));
    }
    Ok(msg)
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

fn tool_ingest_notes(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let notes_dir = get_str(args, "notes").map(PathBuf::from).ok_or_else(|| anyhow::anyhow!("missing required 'notes' argument"))?;
    let dry_run = args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
    let llm_match = args.get("llm_match").and_then(|v| v.as_bool()).unwrap_or(false);
    let min_name_len = args.get("min_name_len").and_then(|v| v.as_u64()).unwrap_or(4) as usize;

    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let repo_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_string();

    let notes = agentops_notes::walk_vault(&notes_dir)?;

    let cheap_matcher = agentops_notes::WordBoundaryMatcher { min_name_len };
    let llm_config = if llm_match { Some(agentops_llm::AnthropicConfig::from_env()?) } else { None };
    let llm_matcher = llm_config.as_ref().map(|config| agentops_llm::LlmAssistedMatcher { config, min_name_len });
    let matcher: &dyn agentops_notes::SymbolMatcher = match &llm_matcher {
        Some(m) => m,
        None => &cheap_matcher,
    };

    if dry_run {
        let mut out = format!("Found {} notes under {}\n\n", notes.len(), notes_dir.display());
        for note in &notes {
            let matched_ids = matcher.match_symbols(&store, &repo_name, &note.body)?;
            let names: Vec<String> = matched_ids.iter().filter_map(|&id| store.get_node(id).ok().flatten().and_then(|n| n.name)).collect();
            out.push_str(&format!("[{:?}] {} -> {}\n", note.note_type.node_kind(), note.title, if names.is_empty() { "(no match)".to_string() } else { names.join(", ") }));
        }
        out.push_str("\n(dry run — nothing written)");
        return Ok(out);
    }

    let summary = agentops_notes::ingest_vault(&store, &repo_name, &notes, matcher)?;
    Ok(format!("Ingested {} of {} notes, wrote {} Affects edge(s).", summary.notes_written, summary.notes_seen, summary.edges_written))
}

fn tool_explain_symbol(args: &Value) -> anyhow::Result<String> {
    let path = require_path(args)?;
    let db_path = graph_db_path(&path);
    if !db_path.exists() {
        anyhow::bail!("no graph store at {} — call scan_repo first", db_path.display());
    }
    let store = SqliteGraphStore::open(&db_path)?;
    let repo_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_string();

    let symbol_id = match args.get("symbol_id").and_then(|v| v.as_i64()) {
        Some(id) => id,
        None => {
            let name = get_str(args, "symbol_name").ok_or_else(|| anyhow::anyhow!("provide either symbol_id or symbol_name"))?;
            let file = get_str(args, "file").map(std::path::PathBuf::from);
            agentops_llm::find_symbol_by_name(&store, &repo_name, name, file.as_deref())?
        }
    };

    let config = agentops_llm::AnthropicConfig::from_env()?;
    let definition_id = agentops_llm::explain_symbol(&store, &config, symbol_id)?;
    let definition = store.get_node(definition_id)?.ok_or_else(|| anyhow::anyhow!("definition node vanished immediately after being written"))?;

    Ok(format!(
        "Recorded definition #{definition_id} for symbol #{symbol_id} ({}):\n\n{}",
        definition.name.as_deref().unwrap_or("<unnamed>"),
        definition.content.as_deref().unwrap_or("")
    ))
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
    fn get_changelog_reports_the_latest_scans_diff_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        call_tool(AccessMode::Full, "scan_repo", &json!({"path": root})).unwrap();

        let changelog = call_tool(AccessMode::Full, "get_changelog", &json!({"path": root})).unwrap();
        assert!(!changelog.is_error, "{:?}", changelog.content);
        let text = &changelog.content[0].text;
        assert!(text.contains("Files: +1"));
        assert!(text.contains("Symbols: +1"));
        assert!(text.contains("added symbol"));
        assert!(text.contains("greet"));
    }

    #[test]
    fn get_changelog_with_no_scans_yet_says_so_instead_of_erroring() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.py"), "x = 1\n").unwrap();
        // Scan once so .context/graph.db exists, but query a since_scan_id
        // that was never recorded to exercise the "not found" path distinctly
        // from "no graph store at all".
        call_tool(AccessMode::Full, "scan_repo", &json!({"path": root})).unwrap();

        let result = call_tool(AccessMode::Full, "get_changelog", &json!({"path": root, "since_scan_id": 999}));
        assert!(result.is_err() || result.unwrap().is_error, "an unknown scan id must be a clear error, not a panic or empty success");
    }

    #[test]
    fn explain_symbol_is_a_write_tool_hidden_in_advisor_mode() {
        let advisor_names: Vec<&str> = list_tools(AccessMode::Advisor).iter().map(|t| t.name).collect();
        assert!(!advisor_names.contains(&"explain_symbol"));
        let full_names: Vec<&str> = list_tools(AccessMode::Full).iter().map(|t| t.name).collect();
        assert!(full_names.contains(&"explain_symbol"));
    }

    #[test]
    fn explain_symbol_without_an_api_key_is_a_clear_tool_error_not_a_panic() {
        // SAFETY: no other test in this crate reads or writes this env var,
        // and cargo test runs each crate's tests in one process but the
        // suite doesn't parallelize across env-var-sensitive tests here.
        unsafe { std::env::remove_var("AGENTOPS_ANTHROPIC_API_KEY") };

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.py"), "def verify_token(t):\n    return True\n").unwrap();
        call_tool(AccessMode::Full, "scan_repo", &json!({"path": root})).unwrap();

        let result = call_tool(AccessMode::Full, "explain_symbol", &json!({"path": root, "symbol_name": "verify_token"})).unwrap();
        assert!(result.is_error);
        assert!(result.content[0].text.contains("AGENTOPS_ANTHROPIC_API_KEY"));
    }

    #[test]
    fn ingest_notes_dry_run_reports_matches_without_writing() {
        let repo_dir = tempfile::tempdir().unwrap();
        fs::write(repo_dir.path().join("auth.py"), "def verify_token(t):\n    return True\n").unwrap();
        call_tool(AccessMode::Full, "scan_repo", &json!({"path": repo_dir.path()})).unwrap();

        let notes_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(notes_dir.path().join("gotchas")).unwrap();
        fs::write(notes_dir.path().join("gotchas/token.md"), "---\ntitle: token bug\n---\n\nverify_token has an off-by-one bug.\n").unwrap();

        let result = call_tool(AccessMode::Full, "ingest_notes", &json!({"path": repo_dir.path(), "notes": notes_dir.path(), "dry_run": true})).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("verify_token"));
        assert!(result.content[0].text.contains("dry run"));

        let gotchas = call_tool(AccessMode::Full, "list_gotchas", &json!({"path": repo_dir.path()})).unwrap();
        assert!(!gotchas.content[0].text.contains("token bug"), "dry_run must not write anything");
    }

    #[test]
    fn ingest_notes_for_real_writes_gotcha_nodes_connected_to_matched_symbols() {
        let repo_dir = tempfile::tempdir().unwrap();
        fs::write(repo_dir.path().join("auth.py"), "def verify_token(t):\n    return True\n").unwrap();
        call_tool(AccessMode::Full, "scan_repo", &json!({"path": repo_dir.path()})).unwrap();

        let notes_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(notes_dir.path().join("gotchas")).unwrap();
        fs::write(notes_dir.path().join("gotchas/token.md"), "---\ntitle: token bug\n---\n\nverify_token has an off-by-one bug.\n").unwrap();

        let result = call_tool(AccessMode::Full, "ingest_notes", &json!({"path": repo_dir.path(), "notes": notes_dir.path()})).unwrap();
        assert!(!result.is_error, "{:?}", result.content);
        assert!(result.content[0].text.contains("Ingested 1 of 1 notes"));

        let gotchas = call_tool(AccessMode::Full, "list_gotchas", &json!({"path": repo_dir.path()})).unwrap();
        assert!(gotchas.content[0].text.contains("token bug"));
        assert!(gotchas.content[0].text.contains("verify_token"), "the gotcha must be connected to the matched symbol: {:?}", gotchas.content);
    }

    #[test]
    fn unknown_tool_name_is_rejected() {
        let result = call_tool(AccessMode::Full, "delete_everything", &json!({}));
        assert!(result.is_err());
    }
}
