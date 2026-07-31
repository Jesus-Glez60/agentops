//! The single, shared "scan a repo and persist it to the graph store"
//! implementation — used by both `tool_scan_repo` (the MCP tool an agent
//! actually calls mid-session) and `agentops-cli`'s `install` command (the
//! human-run equivalent). Deliberately factored out rather than left as two
//! independent copies: it used to be two copies, and they drifted — the CLI
//! path got upsert/prune/DependsOn-edge support (fixing a real
//! duplicate-node-on-rescan bug) while this crate's `tool_scan_repo` quietly
//! kept the old, buggy `add_node`-only behavior, meaning the *actual*
//! primary way this product gets used (an agent calling `scan_repo` via
//! MCP) still accumulated duplicate nodes on every rescan even after the
//! bug was supposedly fixed. One implementation, called from both places,
//! is what actually prevents that class of bug from coming back.

use std::path::Path;

use agentops_graph::{EdgeRelation, GraphStore, NewNode, NodeKind, SqliteGraphStore};
use agentops_scanner::ScanReport;

pub struct ScanPersistSummary {
    pub files: usize,
    pub symbols: usize,
    pub dependency_edges: usize,
    pub pruned_files: usize,
    pub pruned_symbols: usize,
}

/// Scans `path` (via `agentops_scanner::scan_repo`) and persists it into
/// `.context/graph.db` under `path`: upserts File/Symbol nodes (rescanning
/// an unchanged file/symbol reuses its id rather than duplicating it, so
/// any gotcha/decision edge pointing at a symbol survives a rescan), prunes
/// whatever's left over from a prior scan that this one didn't touch, and
/// persists `DependsOn` edges resolved from each file's imports.
pub fn scan_and_persist(path: &Path) -> anyhow::Result<ScanPersistSummary> {
    let report = agentops_scanner::scan_repo(path)?;
    let summary = persist(path, &report)?;
    Ok(summary)
}

/// The persistence half on its own, for callers that already have a
/// `ScanReport` (e.g. because they scanned once already to print a preview)
/// and don't want to pay for scanning the repo twice.
pub fn persist(path: &Path, report: &ScanReport) -> anyhow::Result<ScanPersistSummary> {
    let repo_name = path
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string());

    let db_path = path.join(".context").join("graph.db");
    let store = SqliteGraphStore::open(&db_path)?;

    let mut kept_file_ids = Vec::with_capacity(report.files.len());
    let mut kept_symbol_ids = Vec::new();
    let mut file_id_by_path = std::collections::HashMap::new();
    let mut symbol_count = 0;
    for file in &report.files {
        let path_str = file.path.to_string_lossy().to_string();
        let file_id = agentops_graph::upsert_node(
            &store,
            NewNode { kind: NodeKind::File, repo: repo_name.clone(), path: Some(path_str.clone()), name: None, start_line: None, end_line: None, content: None },
        )?;
        kept_file_ids.push(file_id);
        file_id_by_path.insert(file.path.clone(), file_id);

        for symbol in &file.symbols {
            let symbol_id = agentops_graph::upsert_node(
                &store,
                NewNode {
                    kind: NodeKind::Symbol,
                    repo: repo_name.clone(),
                    path: Some(path_str.clone()),
                    name: Some(symbol.name.clone()),
                    start_line: Some(symbol.start_line as i64),
                    end_line: Some(symbol.end_line as i64),
                    content: Some(symbol.source.clone()),
                },
            )?;
            kept_symbol_ids.push(symbol_id);
            symbol_count += 1;
        }
    }

    let pruned_files = agentops_graph::prune_stale_nodes(&store, &repo_name, NodeKind::File, &kept_file_ids)?;
    let pruned_symbols = agentops_graph::prune_stale_nodes(&store, &repo_name, NodeKind::Symbol, &kept_symbol_ids)?;

    let dep_edges = agentops_scanner::resolve_dependency_edges(&report.files);
    for file_id in file_id_by_path.values() {
        store.delete_edges_from(*file_id, EdgeRelation::DependsOn)?;
    }
    let mut dependency_edges = 0;
    for (from, to) in &dep_edges {
        if let (Some(&from_id), Some(&to_id)) = (file_id_by_path.get(from), file_id_by_path.get(to)) {
            store.add_edge(from_id, to_id, EdgeRelation::DependsOn)?;
            dependency_edges += 1;
        }
    }

    Ok(ScanPersistSummary { files: report.files.len(), symbols: symbol_count, dependency_edges, pruned_files, pruned_symbols })
}
