//! The single, shared "scan a repo and persist it to the graph store"
//! implementation — used by both the MCP `scan_repo` tool (the one an agent
//! actually calls mid-session) and `agentops-cli`'s `install` command (the
//! human-run equivalent). Deliberately factored out rather than left as two
//! independent copies: `main` had exactly that split, and they drifted —
//! the CLI path got upsert/prune/edge-refresh support (fixing a real
//! duplicate-node-on-rescan bug) while the MCP-tool path quietly kept the
//! old, buggy `add_node`-only behavior, meaning the *actual* primary way
//! this product gets used (an agent calling `scan_repo` mid-session) kept
//! accumulating duplicate nodes even after the bug was "fixed" elsewhere.
//! One implementation, called from both places, is what actually prevents
//! that class of bug from recurring.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use agentops_embeddings::Embedder;
use agentops_graph::{upsert_node, prune_stale_nodes, EdgeRelation, NewNode, NewScanHistoryEntry, NodeKind, ScanChange};
use agentops_scanner::ScanReport;
use anyhow::Result;

pub struct ScanPersistSummary {
    pub files: usize,
    pub symbols: usize,
    pub dependency_edges: usize,
    pub pruned_files: usize,
    pub pruned_symbols: usize,
}

/// `pub` (not just `pub(crate)`) so driving adapters outside this crate
/// (`agentops-cli`'s `status`/`changelog` commands) can open the same store
/// the same way every other adapter does, instead of reimplementing this
/// path-joining logic a second time — a duplication risk otherwise.
pub fn graph_db_path(repo_path: &Path) -> PathBuf {
    repo_path.join(".context").join("graph.db")
}

/// The repo's identity within the graph store — canonicalized directory
/// name, falling back to the raw path string if canonicalization fails
/// (e.g. the path doesn't exist yet in a test). `pub` for the same reason
/// as `graph_db_path`.
pub fn repo_name(path: &Path) -> String {
    path.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| path.display().to_string())
}

/// Scans `path` (via `agentops_scanner::scan_repo`) and persists it into
/// `.context/graph.db` under `path`. `with_embeddings` is opt-in — see
/// `persist`'s doc comment.
pub fn scan_and_persist(path: &Path, with_embeddings: bool) -> Result<ScanPersistSummary> {
    let report = agentops_scanner::scan_repo(path)?;
    persist(path, &report, with_embeddings)
}

/// The persistence half on its own, for callers that already have a
/// `ScanReport` (e.g. because they scanned once already to print a preview)
/// and don't want to pay for scanning the repo twice.
///
/// `with_embeddings`, deliberately opt-in (matching `explain_symbol`'s
/// "never run automatically" philosophy — embedding is local/free of API
/// cost but still real CPU-bound latency per symbol, not something every
/// scan should pay by default): when true, each new/changed symbol's
/// content is embedded via `agentops_embeddings::LocalEmbedder` and
/// attached via `GraphStore::set_embedding` right after its `upsert_node`
/// call, making it findable via `search_similar`/the `semantic_search` tool.
pub fn persist(path: &Path, report: &ScanReport, with_embeddings: bool) -> Result<ScanPersistSummary> {
    let repo = repo_name(path);
    let store = crate::store::open_store(path)?;

    let mut kept_file_ids = Vec::with_capacity(report.files.len());
    let mut kept_symbol_ids = Vec::new();
    let mut file_id_by_path: HashMap<PathBuf, i64> = HashMap::new();
    let mut symbol_count = 0;

    let mut scan_entries: Vec<NewScanHistoryEntry> = Vec::new();
    let mut new_file_ids: HashSet<i64> = HashSet::new();
    let mut files_with_symbol_changes: HashSet<i64> = HashSet::new();

    for file in &report.files {
        let path_str = file.path.to_string_lossy().to_string();
        let existing_file = store.find_node(&repo, NodeKind::File, Some(&path_str), None, None)?;
        let file_id = upsert_node(
            store.as_ref(),
            NewNode { kind: NodeKind::File, repo: repo.clone(), path: Some(path_str.clone()), name: None, container: None, start_line: None, end_line: None, content: None },
        )?;
        kept_file_ids.push(file_id);
        file_id_by_path.insert(file.path.clone(), file_id);
        if existing_file.is_none() {
            new_file_ids.insert(file_id);
            scan_entries.push(NewScanHistoryEntry { node_id: file_id, kind: NodeKind::File, path: Some(path_str.clone()), name: None, change: ScanChange::Added });
        }

        for symbol in &file.symbols {
            let existing_symbol = store.find_node(&repo, NodeKind::Symbol, Some(&path_str), Some(&symbol.name), symbol.container.as_deref())?;
            let symbol_id = upsert_node(
                store.as_ref(),
                NewNode {
                    kind: NodeKind::Symbol,
                    repo: repo.clone(),
                    path: Some(path_str.clone()),
                    name: Some(symbol.name.clone()),
                    container: symbol.container.clone(),
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
                scan_entries.push(NewScanHistoryEntry { node_id: symbol_id, kind: NodeKind::Symbol, path: Some(path_str.clone()), name: Some(symbol.name.clone()), change });
                files_with_symbol_changes.insert(file_id);

                // Only new/changed symbols need (re)embedding — an unchanged
                // symbol's embedding from a prior scan is still accurate.
                if with_embeddings {
                    let embedding = agentops_embeddings::LocalEmbedder.embed(&symbol.source)?;
                    store.set_embedding(&repo, symbol_id, &embedding)?;
                }
            }
        }
    }

    // A File node never carries `content`, so "file changed" can't be
    // detected via content-diff the way a symbol's can — instead a file
    // counts as `Changed` if any of its own symbols were added/changed
    // this scan, unless the file itself was already recorded `Added`.
    for &file_id in &files_with_symbol_changes {
        if new_file_ids.contains(&file_id) {
            continue;
        }
        if let Some((path_buf, _)) = file_id_by_path.iter().find(|(_, &id)| id == file_id) {
            scan_entries.push(NewScanHistoryEntry { node_id: file_id, kind: NodeKind::File, path: Some(path_buf.to_string_lossy().to_string()), name: None, change: ScanChange::Changed });
        }
    }

    // Repo-scoped by construction (Module A's `GraphStore`), so this prune
    // can never leak into a different repo's nodes even for a shared store.
    let pruned_files = prune_stale_nodes(store.as_ref(), &repo, NodeKind::File, &kept_file_ids)?;
    let pruned_symbols = prune_stale_nodes(store.as_ref(), &repo, NodeKind::Symbol, &kept_symbol_ids)?;
    for f in &pruned_files {
        scan_entries.push(NewScanHistoryEntry { node_id: f.id, kind: NodeKind::File, path: f.path.clone(), name: None, change: ScanChange::Removed });
    }
    for s in &pruned_symbols {
        scan_entries.push(NewScanHistoryEntry { node_id: s.id, kind: NodeKind::Symbol, path: s.path.clone(), name: s.name.clone(), change: ScanChange::Removed });
    }

    // DependsOn edges are fully replaced per touched file each scan, not diffed.
    let dep_edges = agentops_scanner::resolve_dependency_edges(path, &report.files);
    for &file_id in file_id_by_path.values() {
        store.delete_edges_from(&repo, file_id, EdgeRelation::DependsOn)?;
    }
    let mut dependency_edges = 0;
    for (from, to) in &dep_edges {
        if let (Some(&from_id), Some(&to_id)) = (file_id_by_path.get(from), file_id_by_path.get(to)) {
            store.add_edge(&repo, from_id, to_id, EdgeRelation::DependsOn)?;
            dependency_edges += 1;
        }
    }

    store.record_scan(&repo, &scan_entries)?;

    // Module B: auto re-match already-written notes against this scan's
    // current symbols — closes a real, previously-documented gap
    // (`.agentops/notes/notes-written-before-first-scan-never-attach-to-
    // symbols.md`) where a note's `Affects` edges only ever formed at write
    // time and never automatically refreshed on a later scan. Default-on,
    // unlike `with_embeddings`: this is pure in-process regex matching over
    // already-loaded data (no network/LLM cost), cheap enough not to gate
    // behind a flag. A repo with no notes yet is a fast no-op (one
    // `is_dir` check).
    let notes_dir = agentops_notes::resolve_notes_path(path, None);
    if notes_dir.is_dir() {
        let classifier = agentops_notes::HeuristicClassifier;
        let matcher = agentops_notes::WordBoundaryMatcher::default();
        let notes = agentops_notes::walk_vault(&notes_dir, &classifier)?;
        agentops_notes::ingest_vault(store.as_ref(), &repo, &notes, &matcher)?;
    }

    // Module C: refresh the repo-state snapshot now that both this scan and
    // the note re-match above are reflected in the graph — same hook point,
    // no new trigger/schedule needed.
    store.refresh_repo_state(&repo)?;

    Ok(ScanPersistSummary { files: report.files.len(), symbols: symbol_count, dependency_edges, pruned_files: pruned_files.len(), pruned_symbols: pruned_symbols.len() })
}

#[cfg(test)]
mod tests {
    use agentops_graph::{GraphStore, SqliteGraphStore};

    use super::*;

    fn open_store(repo_dir: &Path) -> SqliteGraphStore {
        SqliteGraphStore::open(&graph_db_path(repo_dir)).unwrap()
    }

    #[test]
    fn a_first_scan_records_every_file_and_symbol_as_added() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();

        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let scan = store.latest_scan(&repo).unwrap().expect("a scan was recorded");
        assert_eq!(scan.files_added, 1);
        assert_eq!(scan.symbols_added, 1);
        assert_eq!(scan.files_changed, 0);
        assert_eq!(scan.symbols_changed, 0);

        let entries = store.scan_entries(scan.id).unwrap();
        assert!(entries.iter().any(|e| e.kind == NodeKind::Symbol && e.name.as_deref() == Some("greet") && e.change == ScanChange::Added));
    }

    #[test]
    fn rescanning_an_unchanged_file_records_nothing_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let scan = store.latest_scan(&repo).unwrap().unwrap();
        assert_eq!(scan.files_added, 0, "an unchanged file must not be recorded as added again");
        assert_eq!(scan.symbols_added, 0);
        assert_eq!(scan.files_changed, 0);
        assert_eq!(scan.symbols_changed, 0, "identical content on rescan must not be classified as changed");

        let store_all_nodes = store.all_nodes(&repo).unwrap();
        assert_eq!(store_all_nodes.iter().filter(|n| n.kind == NodeKind::Symbol).count(), 1, "rescanning must not duplicate the symbol node");
    }

    #[test]
    fn rescanning_after_editing_a_symbol_records_it_and_its_file_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hello'\n").unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let scan = store.latest_scan(&repo).unwrap().unwrap();
        assert_eq!(scan.symbols_changed, 1);
        assert_eq!(scan.files_changed, 1, "a file whose symbol changed must itself roll up to changed, not stay silent");

        let entries = store.scan_entries(scan.id).unwrap();
        assert!(entries.iter().any(|e| e.kind == NodeKind::Symbol && e.change == ScanChange::Changed));
        assert!(entries.iter().any(|e| e.kind == NodeKind::File && e.change == ScanChange::Changed));
    }

    #[test]
    fn removing_a_file_records_its_symbols_and_the_file_itself_as_removed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        std::fs::remove_file(dir.path().join("main.py")).unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let scan = store.latest_scan(&repo).unwrap().unwrap();
        assert_eq!(scan.files_removed, 1);
        assert_eq!(scan.symbols_removed, 1);

        let entries = store.scan_entries(scan.id).unwrap();
        assert!(entries.iter().any(|e| e.kind == NodeKind::Symbol && e.name.as_deref() == Some("greet") && e.change == ScanChange::Removed));
    }

    /// Regression test for a confirmed real bug found via live testing
    /// against this actual repo (`agentops-graph/src/lib.rs` defines
    /// `as_db_str` in three separate `impl` blocks): before `container`
    /// disambiguation, two same-named methods in different `impl` blocks
    /// in one file silently collapsed into a single persisted node.
    #[test]
    fn same_name_methods_in_different_impl_blocks_both_persist() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "impl Foo {\n    fn new() -> Self { Foo }\n}\n\nimpl Bar {\n    fn new() -> Self { Bar }\n}\n").unwrap();

        let summary = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary.symbols, 2, "both `new` methods must be counted, not collapsed");

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let symbols = store.nodes_by_kind(&repo, NodeKind::Symbol).unwrap();
        assert_eq!(symbols.len(), 2, "found: {symbols:?}");
        let containers: std::collections::HashSet<_> = symbols.iter().map(|n| n.container.as_deref()).collect();
        assert_eq!(containers, std::collections::HashSet::from([Some("Foo"), Some("Bar")]));
    }

    /// Module B's actual bug-fix proof: a note written against a repo
    /// *before* it's ever been scanned (so there were zero symbols to match
    /// against at write time — the exact real gap recorded as
    /// `.agentops/notes/notes-written-before-first-scan-never-attach-to-
    /// symbols.md` in this repo's own graph) must end up attached with no
    /// manual `ingest-notes` step, purely from the next scan running.
    #[test]
    fn a_note_written_before_the_first_scan_gets_auto_attached_on_the_next_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();

        let notes_dir = dir.path().join(".agentops").join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("token-bug.md"), "---\ntitle: \"Token bug\"\ntype: gotcha\n---\n\nverify_token has a known workaround for a bug.\n").unwrap();

        // No scan has ever run yet — the note above was written completely
        // blind to any symbol. This first scan is what must auto-attach it.
        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let gotcha = store.nodes_by_kind(&repo, NodeKind::Gotcha).unwrap().into_iter().next().expect("the note must have been ingested");
        let edges: Vec<_> = store.edges_from(&repo, gotcha.id).unwrap().into_iter().filter(|e| e.relation == EdgeRelation::Affects).collect();
        assert_eq!(edges.len(), 1, "must be attached to verify_token with no manual ingest-notes step: {edges:?}");
    }

    /// Module C: a scan must leave a fresh, queryable `repo_state` snapshot
    /// behind — no separate refresh step required.
    #[test]
    fn scan_and_persist_refreshes_repo_state() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let notes_dir = dir.path().join(".agentops").join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("token-bug.md"), "---\ntitle: \"Token bug\"\ntype: gotcha\n---\n\nverify_token has a known workaround for a bug.\n").unwrap();

        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let state = store.get_repo_state(&repo).unwrap().expect("persist() must refresh repo_state, not leave it unset");
        assert_eq!(state.top_gotcha_ids.len(), 1);
    }

    #[test]
    fn dependency_edges_are_fully_replaced_not_accumulated_on_rescan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/util.ts"), "export function greet() { return 'hi'; }\n").unwrap();
        std::fs::write(dir.path().join("src/app.ts"), "import { greet } from './util';\nexport function main() { return greet(); }\n").unwrap();

        let summary1 = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary1.dependency_edges, 1);
        let summary2 = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary2.dependency_edges, 1, "rescanning identical imports must not accumulate duplicate edges");

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        assert_eq!(store.all_edges(&repo).unwrap().len(), 1);
    }
}
