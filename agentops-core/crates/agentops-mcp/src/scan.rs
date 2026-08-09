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
use std::process::Command;
use std::time::SystemTime;

use agentops_graph::{
    EdgeRelation, GraphStore, NewNode, NewScanHistoryEntry, NodeKind, ScanChange, ScanEntryKind, SqliteGraphStore,
};
use agentops_scanner::ScanReport;

pub struct ScanPersistSummary {
    pub files: usize,
    pub symbols: usize,
    pub dependency_edges: usize,
    pub pruned_files: usize,
    pub pruned_symbols: usize,
}

/// `git rev-parse HEAD` in `path`, or `None` if `path` isn't a git worktree
/// (or `git` isn't on PATH) -- best-effort, never fails the scan over it.
/// This is the first git shell-out in the light tier; `agentops-repo-access`
/// (heavy tier, a separate commercial workspace this crate can't depend on)
/// has the only other git shell-out in the monorepo, for SSH-authenticated
/// clone/fetch -- much heavier machinery than this. Only the error-handling
/// shape (`Command::new("git").args(...).output()`, non-zero exit -> `None`)
/// is borrowed from it, not shared code.
fn current_git_sha(path: &Path) -> Option<String> {
    let output = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    let sha = sha.trim();
    if sha.is_empty() { None } else { Some(sha.to_string()) }
}

fn now_rfc3339() -> String {
    // No chrono dependency in this crate -- a plain Unix-seconds string is
    // enough for a changelog timestamp (matches agentops-manifest's own
    // "plain number, callers format for display" choice for the same reason).
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs().to_string()).unwrap_or_else(|_| "0".to_string())
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
    let started_at = now_rfc3339();
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
    // Changelog bookkeeping: File nodes never carry `content` (see
    // agentops-graph), so "file changed" can't be detected the same way a
    // symbol's content diff works -- instead a file counts as `changed` if
    // any of its own symbols were added/changed/removed this scan, tracked
    // per file id below and rolled up after the symbol loop.
    let mut scan_entries: Vec<NewScanHistoryEntry> = Vec::new();
    let mut new_file_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut files_with_symbol_changes: std::collections::HashSet<i64> = std::collections::HashSet::new();

    for file in &report.files {
        let path_str = file.path.to_string_lossy().to_string();
        let existing_file = store.find_node(&repo_name, NodeKind::File, Some(&path_str), None)?;
        let file_id = agentops_graph::upsert_node(
            &store,
            NewNode { kind: NodeKind::File, repo: repo_name.clone(), path: Some(path_str.clone()), name: None, start_line: None, end_line: None, content: None },
        )?;
        kept_file_ids.push(file_id);
        file_id_by_path.insert(file.path.clone(), file_id);
        if existing_file.is_none() {
            new_file_ids.insert(file_id);
            scan_entries.push(NewScanHistoryEntry {
                node_id: file_id,
                kind: ScanEntryKind::File,
                path: Some(path_str.clone()),
                name: None,
                change: ScanChange::Added,
            });
        }

        for symbol in &file.symbols {
            let existing_symbol = store.find_node(&repo_name, NodeKind::Symbol, Some(&path_str), Some(&symbol.name))?;
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

            let change = match &existing_symbol {
                None => Some(ScanChange::Added),
                Some(existing) if existing.content.as_deref() != Some(symbol.source.as_str()) => Some(ScanChange::Changed),
                Some(_) => None,
            };
            if let Some(change) = change {
                scan_entries.push(NewScanHistoryEntry {
                    node_id: symbol_id,
                    kind: ScanEntryKind::Symbol,
                    path: Some(path_str.clone()),
                    name: Some(symbol.name.clone()),
                    change,
                });
                files_with_symbol_changes.insert(file_id);
            }
        }
    }

    for &file_id in &files_with_symbol_changes {
        if new_file_ids.contains(&file_id) {
            continue; // already recorded as `added`, not also `changed`
        }
        if let Some((path_buf, _)) = file_id_by_path.iter().find(|(_, &id)| id == file_id) {
            scan_entries.push(NewScanHistoryEntry {
                node_id: file_id,
                kind: ScanEntryKind::File,
                path: Some(path_buf.to_string_lossy().to_string()),
                name: None,
                change: ScanChange::Changed,
            });
        }
    }

    let pruned_files = agentops_graph::prune_stale_nodes(&store, &repo_name, NodeKind::File, &kept_file_ids)?;
    let pruned_symbols = agentops_graph::prune_stale_nodes(&store, &repo_name, NodeKind::Symbol, &kept_symbol_ids)?;
    for f in &pruned_files {
        scan_entries.push(NewScanHistoryEntry { node_id: f.id, kind: ScanEntryKind::File, path: f.path.clone(), name: None, change: ScanChange::Removed });
    }
    for s in &pruned_symbols {
        scan_entries.push(NewScanHistoryEntry { node_id: s.id, kind: ScanEntryKind::Symbol, path: s.path.clone(), name: s.name.clone(), change: ScanChange::Removed });
    }

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

    let git_sha = current_git_sha(path);
    let finished_at = now_rfc3339();
    // notes_added stays 0 here deliberately -- ad-hoc `agentops note` calls
    // don't belong to any scan; only batch operations that own their own
    // start/end (e.g. Phase 2's vault ingestion) record their own count.
    store.record_scan(&repo_name, &started_at, &finished_at, git_sha.as_deref(), &scan_entries, 0)?;

    Ok(ScanPersistSummary {
        files: report.files.len(),
        symbols: symbol_count,
        dependency_edges,
        pruned_files: pruned_files.len(),
        pruned_symbols: pruned_symbols.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_store(repo_dir: &Path) -> SqliteGraphStore {
        SqliteGraphStore::open(&repo_dir.join(".context").join("graph.db")).unwrap()
    }

    #[test]
    fn a_first_scan_records_every_file_and_symbol_as_added() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();

        scan_and_persist(dir.path()).unwrap();

        let store = open_store(dir.path());
        let repo_name = dir.path().file_name().unwrap().to_string_lossy().to_string();
        let scan = store.latest_scan(&repo_name).unwrap().expect("a scan was recorded");
        assert_eq!(scan.files_added, 1);
        assert_eq!(scan.symbols_added, 1);
        assert_eq!(scan.files_changed, 0);
        assert_eq!(scan.symbols_changed, 0);

        let diff = store.scan_diff(scan.id).unwrap();
        assert!(diff.iter().any(|e| e.kind == "symbol" && e.name.as_deref() == Some("greet") && e.change == "added"));
    }

    #[test]
    fn rescanning_an_unchanged_file_records_nothing_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        scan_and_persist(dir.path()).unwrap();

        scan_and_persist(dir.path()).unwrap();

        let store = open_store(dir.path());
        let repo_name = dir.path().file_name().unwrap().to_string_lossy().to_string();
        let scan = store.latest_scan(&repo_name).unwrap().unwrap();
        assert_eq!(scan.files_added, 0, "an unchanged file must not be recorded as added again");
        assert_eq!(scan.symbols_added, 0);
        assert_eq!(scan.files_changed, 0);
        assert_eq!(scan.symbols_changed, 0, "identical content on rescan must not be classified as changed");
    }

    #[test]
    fn rescanning_after_editing_a_symbol_records_it_and_its_file_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        scan_and_persist(dir.path()).unwrap();

        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hello'\n").unwrap();
        scan_and_persist(dir.path()).unwrap();

        let store = open_store(dir.path());
        let repo_name = dir.path().file_name().unwrap().to_string_lossy().to_string();
        let scan = store.latest_scan(&repo_name).unwrap().unwrap();
        assert_eq!(scan.symbols_changed, 1);
        assert_eq!(scan.files_changed, 1, "a file whose symbol changed must itself roll up to changed, not stay silent");

        let diff = store.scan_diff(scan.id).unwrap();
        assert!(diff.iter().any(|e| e.kind == "symbol" && e.change == "changed"));
        assert!(diff.iter().any(|e| e.kind == "file" && e.change == "changed"));
    }

    #[test]
    fn removing_a_file_records_its_symbols_and_the_file_itself_as_removed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        scan_and_persist(dir.path()).unwrap();

        std::fs::remove_file(dir.path().join("main.py")).unwrap();
        scan_and_persist(dir.path()).unwrap();

        let store = open_store(dir.path());
        let repo_name = dir.path().file_name().unwrap().to_string_lossy().to_string();
        let scan = store.latest_scan(&repo_name).unwrap().unwrap();
        assert_eq!(scan.files_removed, 1);
        assert_eq!(scan.symbols_removed, 1);

        let diff = store.scan_diff(scan.id).unwrap();
        assert!(diff.iter().any(|e| e.kind == "symbol" && e.name.as_deref() == Some("greet") && e.change == "removed"));
    }
}
