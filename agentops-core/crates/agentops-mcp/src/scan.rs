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
use agentops_graph::{prune_stale_nodes, EdgeRelation, NewNode, NewScanHistoryEntry, NodeKind, ScanChange};
use agentops_scanner::ScanReport;
use anyhow::Result;

pub struct ScanPersistSummary {
    pub files: usize,
    pub symbols: usize,
    pub dependency_edges: usize,
    /// Same-file symbol-to-symbol `References` edges (AST-precise where
    /// tree-sitter parsed the file, word-boundary fallback otherwise) --
    /// see `agentops_scanner::resolve_same_file_symbol_references`.
    pub reference_edges: usize,
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
    // Same-file symbol `References` pairs, accumulated across every file --
    // see the block after the phase-1/2 loops for how each path is chosen.
    let mut reference_pairs: Vec<(i64, i64)> = Vec::new();

    // -- Phase 1: files, one batched find + one batched upsert for the
    // whole scan instead of one round trip per file. --
    let file_path_strs: Vec<String> = report.files.iter().map(|f| f.path.to_string_lossy().to_string()).collect();
    let file_keys: Vec<(NodeKind, Option<&str>, Option<&str>, Option<&str>)> = file_path_strs.iter().map(|p| (NodeKind::File, Some(p.as_str()), None, None)).collect();
    let existing_files = store.find_nodes_batch(&repo, &file_keys)?;
    let file_new_nodes: Vec<NewNode> =
        file_path_strs.iter().map(|p| NewNode { kind: NodeKind::File, repo: repo.clone(), path: Some(p.clone()), name: None, container: None, start_line: None, end_line: None, content: None }).collect();
    let file_ids = store.upsert_nodes_batch(&file_new_nodes)?;

    for (file, (path_str, &file_id)) in report.files.iter().zip(file_path_strs.iter().zip(&file_ids)) {
        kept_file_ids.push(file_id);
        file_id_by_path.insert(file.path.clone(), file_id);
        if !existing_files.contains_key(&(NodeKind::File, Some(path_str.clone()), None, None)) {
            new_file_ids.insert(file_id);
            scan_entries.push(NewScanHistoryEntry { node_id: file_id, kind: NodeKind::File, path: Some(path_str.clone()), name: None, change: ScanChange::Added });
        }
    }

    // -- Phase 2: symbols, same batched-find + batched-upsert shape across
    // every file's symbols in one pass (not one call per file). Per-symbol
    // content-diff classification (Added/Changed/None) stays an in-memory
    // loop over the already-fetched `existing_symbols` map -- free, not a
    // query. --
    let mut symbol_keys: Vec<(NodeKind, Option<&str>, Option<&str>, Option<&str>)> = Vec::new();
    let mut symbol_new_nodes: Vec<NewNode> = Vec::new();
    for (file, path_str) in report.files.iter().zip(&file_path_strs) {
        for symbol in &file.symbols {
            symbol_keys.push((NodeKind::Symbol, Some(path_str.as_str()), Some(symbol.name.as_str()), symbol.container.as_deref()));
            symbol_new_nodes.push(NewNode {
                kind: NodeKind::Symbol,
                repo: repo.clone(),
                path: Some(path_str.clone()),
                name: Some(symbol.name.clone()),
                container: symbol.container.clone(),
                start_line: Some(symbol.start_line as i64),
                end_line: Some(symbol.end_line as i64),
                content: Some(symbol.source.clone()),
            });
        }
    }
    let existing_symbols = store.find_nodes_batch(&repo, &symbol_keys)?;
    let symbol_ids = store.upsert_nodes_batch(&symbol_new_nodes)?;

    // -- Phase 3: versions + embeddings for changed symbols, one batched
    // call each instead of one per symbol. Embedding computation itself is
    // CPU-bound (no DB round trip), so it stays an in-loop call; only the
    // two `GraphStore` writes at the end are batched. --
    let mut changed_versions: Vec<(i64, Option<String>, Option<i64>, Option<i64>)> = Vec::new();
    let mut changed_embeddings: Vec<(i64, Vec<f32>)> = Vec::new();

    let mut symbol_idx = 0;
    for (file, path_str) in report.files.iter().zip(&file_path_strs) {
        let mut file_symbol_ids: Vec<i64> = Vec::with_capacity(file.symbols.len());
        for symbol in &file.symbols {
            let symbol_id = symbol_ids[symbol_idx];
            symbol_idx += 1;
            kept_symbol_ids.push(symbol_id);
            file_symbol_ids.push(symbol_id);
            symbol_count += 1;

            let existing = existing_symbols.get(&(NodeKind::Symbol, Some(path_str.clone()), Some(symbol.name.clone()), symbol.container.clone()));
            let change = match existing {
                None => Some(ScanChange::Added),
                Some(existing) if existing.content.as_deref() != Some(symbol.source.as_str()) => Some(ScanChange::Changed),
                Some(_) => None,
            };
            if let Some(change) = change {
                let file_id = file_id_by_path[&file.path];
                scan_entries.push(NewScanHistoryEntry { node_id: symbol_id, kind: NodeKind::Symbol, path: Some(path_str.clone()), name: Some(symbol.name.clone()), change });
                files_with_symbol_changes.insert(file_id);

                // Bi-temporal history: records the symbol's *new* content as
                // of now, closing whatever version was previously open (a
                // no-op for a brand-new symbol's first version). Always the
                // new value, never the old one — see
                // GraphStore::snapshot_node_version's doc comment for why.
                changed_versions.push((symbol_id, Some(symbol.source.clone()), Some(symbol.start_line as i64), Some(symbol.end_line as i64)));

                // Only new/changed symbols need (re)embedding — an unchanged
                // symbol's embedding from a prior scan is still accurate.
                if with_embeddings {
                    let embedding = agentops_embeddings::LocalEmbedder.embed(&symbol.source)?;
                    changed_embeddings.push((symbol_id, embedding));
                }
            }
        }

        // Same-file symbol references: AST-precise (via each symbol's
        // pre-collected `references` set) when tree-sitter parsed this
        // file, word-boundary text matching as a fallback when it didn't
        // (no AST to walk in that case). `file_symbol_ids` lines up 1:1
        // with `file.symbols` since both were built in this same loop.
        if file.used_tree_sitter {
            for (from_idx, to_idx) in agentops_scanner::resolve_same_file_symbol_references(&file.symbols) {
                reference_pairs.push((file_symbol_ids[from_idx], file_symbol_ids[to_idx]));
            }
        } else {
            let triples: Vec<(i64, &str, &str)> = file_symbol_ids.iter().zip(&file.symbols).map(|(&id, s)| (id, s.name.as_str(), s.source.as_str())).collect();
            for (from_id, to_id, _) in agentops_notes::match_same_file_references(&triples, 4)? {
                reference_pairs.push((from_id, to_id));
            }
        }
    }
    let symbol_versions_repo = repo.clone();
    store.snapshot_node_versions_batch(&symbol_versions_repo, &changed_versions.iter().map(|(id, content, sl, el)| (*id, content.as_deref(), *sl, *el)).collect::<Vec<_>>())?;
    store.set_embeddings_batch(&repo, &changed_embeddings)?;

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
        // node_versions has no hard FK to nodes (by design — history must
        // survive pruning), so closing after delete_nodes already ran is
        // fine; this just marks "the last known content was valid until
        // removal" rather than leaving the version open forever.
        store.close_node_version(s.id)?;
    }

    // DependsOn edges are fully replaced per touched file each scan, not diffed.
    let dep_edges = agentops_scanner::resolve_dependency_edges(path, &report.files);
    for &file_id in file_id_by_path.values() {
        store.delete_edges_from(&repo, file_id, EdgeRelation::DependsOn)?;
    }
    // -- Phase 4: dependency edges, one batched insert instead of one
    // `add_edge` call per resolved import. `delete_edges_from` stays a loop
    // per touched file (O(files), not O(symbols) -- low-cost relative to the
    // edge-count problem batching actually targets). --
    let mut new_depends_on_edges: Vec<(i64, i64, EdgeRelation)> = Vec::new();
    for (from, to) in &dep_edges {
        if let (Some(&from_id), Some(&to_id)) = (file_id_by_path.get(from), file_id_by_path.get(to)) {
            new_depends_on_edges.push((from_id, to_id, EdgeRelation::DependsOn));
        }
    }
    let dependency_edges = new_depends_on_edges.len();
    store.add_edges_batch(&repo, &new_depends_on_edges)?;

    // -- Phase 5: reference-edge reconciliation, the most complex batched
    // phase. Reference edges are plastic (Initiative 1, CLS-inspired
    // retrieval plan) -- unlike DependsOn above, no longer fully deleted and
    // recreated per touched symbol each scan. A reference re-confirmed this
    // scan reinforces its existing edge (bumping weight, same mechanism a
    // repeat-matched `Affects` edge already uses) instead of resetting to
    // weight 1.0; a reference whose target genuinely disappeared is pruned.
    // One `edges_from_batch` call replaces one `edges_from` call per symbol;
    // the same in-memory reinforce/delete/add decision as before is then
    // applied via one batched call each instead of per-edge calls. --
    let mut new_targets_by_symbol: HashMap<i64, HashSet<i64>> = HashMap::new();
    for (from_id, to_id) in &reference_pairs {
        new_targets_by_symbol.entry(*from_id).or_default().insert(*to_id);
    }
    let existing_edges_by_symbol = store.edges_from_batch(&repo, &kept_symbol_ids)?;

    let mut reinforce_ids: Vec<i64> = Vec::new();
    let mut delete_ids: Vec<i64> = Vec::new();
    let mut new_reference_edges: Vec<(i64, i64, EdgeRelation)> = Vec::new();
    let mut reference_edges = 0;
    for &symbol_id in &kept_symbol_ids {
        let existing: Vec<_> = existing_edges_by_symbol.get(&symbol_id).into_iter().flatten().filter(|e| e.relation == EdgeRelation::References).collect();
        let new_targets = new_targets_by_symbol.get(&symbol_id).cloned().unwrap_or_default();

        for edge in &existing {
            if new_targets.contains(&edge.dst_id) {
                // Still referenced this scan -- reinforce, don't recreate.
                // `bump_confirmed_at: false`, same convention as the
                // automatic note rematch below: this is a passive rescan
                // re-observation, not a deliberate human reconfirmation, so
                // it must not reset the staleness clock `tool_get_symbol`
                // compares against `node_history`.
                reinforce_ids.push(edge.id);
            } else {
                // No longer referenced this scan -- pruned, not left to
                // decay forever as a stale edge.
                delete_ids.push(edge.id);
            }
        }

        let existing_targets: HashSet<i64> = existing.iter().map(|e| e.dst_id).collect();
        for &to_id in &new_targets {
            if !existing_targets.contains(&to_id) {
                new_reference_edges.push((symbol_id, to_id, EdgeRelation::References));
            }
            reference_edges += 1;
        }
    }
    store.reinforce_edges_batch(&repo, &reinforce_ids, false)?;
    store.delete_edges_batch(&repo, &delete_ids)?;
    store.add_edges_batch(&repo, &new_reference_edges)?;

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
        // `bump_confirmed_at: false` — this is a blind keyword rematch, not
        // a human reconfirming the note is still accurate, so it must not
        // reset the staleness clock `tool_get_symbol` compares against
        // `node_history` (see `GraphStore::reinforce_edge`'s doc comment).
        agentops_notes::ingest_vault(store.as_ref(), &repo, &notes, &matcher, false)?;

        // Same post-ingestion embedding step `notes::ingest_notes_dir`
        // already uses for the CLI/MCP bulk path -- `ingest_vault` itself
        // has no embedding step at all, so without this, every automatic
        // rescan (this function) would leave Gotcha/Decision/Note nodes
        // permanently unembedded regardless of `with_embeddings`, unlike
        // Symbol nodes just above. Found via live testing: the search
        // page's kind=gotcha filter returned nothing until this was added.
        //
        // -- Phase 6: note-embedding backfill, one batched `set_embeddings_batch`
        // call instead of one `set_embedding` call per note. --
        if with_embeddings {
            let mut note_embeddings: Vec<(i64, Vec<f32>)> = Vec::new();
            for kind in [NodeKind::Gotcha, NodeKind::Decision, NodeKind::Note] {
                for node in store.nodes_by_kind(&repo, kind)? {
                    if let Some(content) = &node.content {
                        let embedding = agentops_embeddings::LocalEmbedder.embed(content)?;
                        note_embeddings.push((node.id, embedding));
                    }
                }
            }
            store.set_embeddings_batch(&repo, &note_embeddings)?;
        }
    }

    // Module C: refresh the repo-state snapshot now that both this scan and
    // the note re-match above are reflected in the graph — same hook point,
    // no new trigger/schedule needed.
    store.refresh_repo_state(&repo)?;

    // Documentation Viewer: regenerate and persist this repo's doc page.
    // Best-effort — a doc-generation hiccup (or, if `AGENTOPS_ANTHROPIC_API_KEY`
    // isn't set, the LLM module-labeling step failing outright) must never
    // fail the scan itself, so every error here is logged and swallowed.
    if let Err(err) = crate::docgen::persist_doc_page(store.as_ref(), path, &repo, &report.files, with_embeddings) {
        eprintln!("warning: failed to regenerate the documentation page for {repo:?}: {err:#}");
    }

    Ok(ScanPersistSummary { files: report.files.len(), symbols: symbol_count, dependency_edges, reference_edges, pruned_files: pruned_files.len(), pruned_symbols: pruned_symbols.len() })
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

    /// Same invariant as `rescanning_an_unchanged_file_records_nothing_as_changed`
    /// above, but against the real production write path: `persist()`'s own
    /// `crate::store::open_store()` call, with `AGENTOPS_DATABASE_URL` set so
    /// it picks `PostgresGraphStore` -- every other test in this file uses
    /// this module's local `open_store` helper (hardcoded to SQLite), which
    /// exercises the upsert logic but never the actual backend this
    /// deployment runs against. Live against a real local Postgres, matching
    /// `agentops-graph-pg`'s own established discipline; skips (not fails)
    /// when nothing is reachable, so this crate's suite doesn't hard-require
    /// Docker/Postgres on every machine.
    ///
    /// `#[ignore]`d deliberately: `AGENTOPS_DATABASE_URL` is process-global,
    /// and this crate has many other tests (`tools::tests::*`, `docgen::
    /// tests::*`) that call `scan_and_persist`/`open_store` expecting the
    /// SQLite default -- confirmed live that running this test under normal
    /// `cargo test` parallelism intermittently reroutes those unrelated
    /// tests to Postgres mid-run and breaks them, `ENV_LOCK` only
    /// serializes tests that explicitly opt into acquiring it, not every
    /// test that happens to touch `open_store`. Run this one in isolation:
    /// `cargo test -p agentops-mcp -- --ignored --test-threads=1
    /// rescanning_an_unchanged_repo_against_postgres_does_not_duplicate_nodes`.
    #[test]
    #[ignore = "mutates the process-global AGENTOPS_DATABASE_URL env var; run in isolation, see doc comment"]
    fn rescanning_an_unchanged_repo_against_postgres_does_not_duplicate_nodes() {
        let _guard = crate::store::test_support::ENV_LOCK.lock().unwrap();
        let url = std::env::var("AGENTOPS_TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:test@localhost:5433/agentops_test".to_string());
        let Ok(pg_store) = agentops_graph_pg::PostgresGraphStore::connect(&url) else {
            eprintln!("skipping rescanning_an_unchanged_repo_against_postgres_does_not_duplicate_nodes: no Postgres reachable at {url}");
            return;
        };

        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::set_var("AGENTOPS_DATABASE_URL", &url) };

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let repo = repo_name(dir.path());

        let result = (|| -> Result<()> {
            scan_and_persist(dir.path(), false)?;
            scan_and_persist(dir.path(), false)?;

            let nodes = GraphStore::all_nodes(&pg_store, &repo)?;
            assert_eq!(
                nodes.iter().filter(|n| n.kind == NodeKind::File).count(),
                1,
                "a second scan of an unchanged repo must not duplicate the file node against Postgres"
            );
            assert_eq!(
                nodes.iter().filter(|n| n.kind == NodeKind::Symbol).count(),
                1,
                "a second scan of an unchanged repo must not duplicate the symbol node against Postgres"
            );
            Ok(())
        })();

        // Cleanup runs regardless of assertion outcome, then the env var is
        // unset, regardless of outcome -- a panicked assertion above must
        // not leak either test rows or a mutated global env var into
        // whatever test runs next.
        if let Ok(nodes) = GraphStore::all_nodes(&pg_store, &repo) {
            let ids: Vec<i64> = nodes.iter().map(|n| n.id).collect();
            let _ = GraphStore::delete_nodes(&pg_store, &repo, &ids);
        }
        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::remove_var("AGENTOPS_DATABASE_URL") };

        result.unwrap();
    }

    /// Broader companion to the test above -- that one only exercises
    /// Phase 1/2 (files/symbols) against Postgres; this one exercises
    /// Phase 3 (versions/embeddings), Phase 4 (dependency edges), and
    /// Phase 5 (reference-edge reconciliation) too, across an edit and a
    /// rescan, against the real batched `PostgresGraphStore` overrides --
    /// not just the SQLite path every other test in this file covers. Same
    /// `#[ignore]`/`ENV_LOCK` discipline as the test above, for the same
    /// reason (mutates the process-global `AGENTOPS_DATABASE_URL`).
    #[test]
    #[ignore = "mutates the process-global AGENTOPS_DATABASE_URL env var; run in isolation, see doc comment"]
    fn batched_persist_produces_correct_deps_references_versions_and_embeddings_against_postgres() {
        let _guard = crate::store::test_support::ENV_LOCK.lock().unwrap();
        let url = std::env::var("AGENTOPS_TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:test@localhost:5433/agentops_test".to_string());
        let Ok(pg_store) = agentops_graph_pg::PostgresGraphStore::connect(&url) else {
            eprintln!("skipping batched_persist_produces_correct_deps_references_versions_and_embeddings_against_postgres: no Postgres reachable at {url}");
            return;
        };

        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::set_var("AGENTOPS_DATABASE_URL", &url) };

        // Cross-file import (util.ts -> app.ts's DependsOn) plus a
        // same-file symbol call (helper/main in main.rs -- References edges
        // are same-file only, see `symbols_in_different_files_with_matching_
        // names_never_get_a_reference_edge` below, so the cross-file pair
        // above can't exercise Phase 5 on its own).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/util.ts"), "export function greet() { return 'hi'; }\n").unwrap();
        std::fs::write(dir.path().join("src/app.ts"), "import { greet } from './util';\nexport function main() { return greet(); }\n").unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn helper() -> i32 { 1 }\n\nfn caller() -> i32 { helper() }\n").unwrap();
        let repo = repo_name(dir.path());

        let result = (|| -> Result<()> {
            let summary1 = scan_and_persist(dir.path(), true)?;
            assert_eq!(summary1.dependency_edges, 1, "the import must produce one DependsOn edge via the batched Phase 4 path");
            assert_eq!(summary1.reference_edges, 1, "caller calling helper (same file) must produce one References edge via the batched Phase 5 path");

            let nodes = GraphStore::all_nodes(&pg_store, &repo)?;
            let caller_fn = nodes.iter().find(|n| n.name.as_deref() == Some("caller")).expect("caller must exist");
            let helper_fn = nodes.iter().find(|n| n.name.as_deref() == Some("helper")).expect("helper must exist");

            // Phase 3: embeddings actually landed via the batched
            // `set_embeddings_batch` override, not silently skipped.
            assert!(GraphStore::get_embedding(&pg_store, &repo, caller_fn.id)?.is_some(), "caller's embedding must be set after a with_embeddings scan");
            assert!(GraphStore::get_embedding(&pg_store, &repo, helper_fn.id)?.is_some(), "helper's embedding must be set after a with_embeddings scan");

            // Edit helper's body, then rescan -- exercises Phase 3's
            // snapshot_node_versions_batch (a real second version must
            // open) and Phase 5's reinforce path (the still-present
            // reference edge must be reinforced, not recreated at weight 1.0).
            std::fs::write(dir.path().join("main.rs"), "fn helper() -> i32 { 2 }\n\nfn caller() -> i32 { helper() }\n").unwrap();
            let summary2 = scan_and_persist(dir.path(), true)?;
            assert_eq!(summary2.dependency_edges, 1, "rescanning identical imports must not accumulate duplicate DependsOn edges");
            assert_eq!(summary2.reference_edges, 1, "the still-present reference must not be dropped or duplicated on rescan");

            let history = GraphStore::node_history(&pg_store, helper_fn.id)?;
            assert_eq!(history.len(), 2, "editing helper must open a real second version via the batched Phase 3 path: {history:?}");
            assert!(history[0].content.as_deref().unwrap().contains('2'));
            assert!(history[1].valid_until.is_some(), "the original version must have been closed, not left open");

            let all_edges = GraphStore::all_edges(&pg_store, &repo)?;
            let references: Vec<_> = all_edges.iter().filter(|e| e.relation == agentops_graph::EdgeRelation::References).collect();
            assert_eq!(references.len(), 1, "still exactly one reference edge after rescan, not a duplicate");
            assert_eq!(references[0].weight, 1.5, "the still-present reference must be reinforced (Phase 5's batched reinforce path), not recreated at a fresh weight");

            let depends_on: Vec<_> = all_edges.iter().filter(|e| e.relation == agentops_graph::EdgeRelation::DependsOn).collect();
            assert_eq!(depends_on.len(), 1, "still exactly one DependsOn edge after rescan, not accumulated");

            Ok(())
        })();

        // Cleanup runs regardless of assertion outcome, same discipline as
        // the test above.
        if let Ok(nodes) = GraphStore::all_nodes(&pg_store, &repo) {
            let ids: Vec<i64> = nodes.iter().map(|n| n.id).collect();
            let _ = GraphStore::delete_nodes(&pg_store, &repo, &ids);
        }
        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::remove_var("AGENTOPS_DATABASE_URL") };

        result.unwrap();
    }

    /// Reproduces the exact real-world shape that broke production the
    /// first time `scan_and_persist` ran against the real `agentops` repo
    /// after the batched-upsert rewrite: two `impl` blocks for the same
    /// type each defining a method with the same name (`container` only
    /// records the *type* name, not which `impl` block -- see
    /// `agentops-scanner::ast_extract`'s own test coverage, which only
    /// covers *different* types, not two blocks for the *same* type). This
    /// collided `(path, name, container)` and made `PostgresGraphStore`'s
    /// first `upsert_nodes_batch` implementation fail outright ("ON
    /// CONFLICT DO UPDATE command cannot affect row a second time") the
    /// moment a real file with this shape was scanned -- SQLite's simpler
    /// per-row upsert loop never surfaced it (see this file's own
    /// `same_name_methods_in_different_impl_blocks_both_persist`, which
    /// only exercises SQLite). This test is the Postgres-path analog,
    /// confirming `scan_and_persist` -- the single implementation shared by
    /// both the MCP `scan_repo` tool and the REST/indexing-pipeline `rescan`
    /// path -- no longer crashes on this shape against the real backend
    /// this deployment runs.
    #[test]
    #[ignore = "mutates the process-global AGENTOPS_DATABASE_URL env var; run in isolation, see doc comment"]
    fn scanning_two_impl_blocks_with_a_same_named_method_does_not_crash_against_postgres() {
        let _guard = crate::store::test_support::ENV_LOCK.lock().unwrap();
        let url = std::env::var("AGENTOPS_TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:test@localhost:5433/agentops_test".to_string());
        let Ok(pg_store) = agentops_graph_pg::PostgresGraphStore::connect(&url) else {
            eprintln!("skipping scanning_two_impl_blocks_with_a_same_named_method_does_not_crash_against_postgres: no Postgres reachable at {url}");
            return;
        };

        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::set_var("AGENTOPS_DATABASE_URL", &url) };

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lora.rs"),
            "struct Widget;\n\nimpl Widget {\n    pub fn forward(&self, x: i32) -> i32 { x + 1 }\n}\n\nimpl Widget {\n    fn forward(&self, y: i32) -> i32 { y + 2 }\n}\n",
        )
        .unwrap();
        let repo = repo_name(dir.path());

        let result = scan_and_persist(dir.path(), false);

        // Cleanup regardless of outcome, same discipline as this file's
        // other live-Postgres tests.
        if let Ok(nodes) = GraphStore::all_nodes(&pg_store, &repo) {
            let ids: Vec<i64> = nodes.iter().map(|n| n.id).collect();
            let _ = GraphStore::delete_nodes(&pg_store, &repo, &ids);
        }
        // SAFETY: guarded by ENV_LOCK above.
        unsafe { std::env::remove_var("AGENTOPS_DATABASE_URL") };

        assert!(result.is_ok(), "scanning a real same-named-method-across-impl-blocks collision must not crash: {:?}", result.err());
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

    /// Phase 2 (1.0 roadmap): persist() must build real bi-temporal history
    /// across rescans, not just today's scan_history_entries diff summary —
    /// this is what makes node_as_of/node_history answer "what did this
    /// symbol look like before" rather than only "did it change."
    #[test]
    fn rescanning_after_editing_a_symbol_builds_real_version_history() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let symbol_id = store.nodes_by_kind(&repo, NodeKind::Symbol).unwrap().into_iter().find(|n| n.name.as_deref() == Some("greet")).unwrap().id;

        let first_history = store.node_history(symbol_id).unwrap();
        assert_eq!(first_history.len(), 1, "the first scan must open exactly one version");
        assert!(first_history[0].content.as_deref().unwrap().contains("'hi'"));
        assert!(first_history[0].valid_until.is_none(), "the only version so far must still be open");

        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hello'\n").unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        let history = store.node_history(symbol_id).unwrap();
        assert_eq!(history.len(), 2, "editing must open a second version, not overwrite the first");
        assert!(history[0].content.as_deref().unwrap().contains("'hello'"), "most recent first: {history:?}");
        assert!(history[0].valid_until.is_none());
        assert!(history[1].content.as_deref().unwrap().contains("'hi'"), "the old content must survive as closed history: {history:?}");
        assert!(history[1].valid_until.is_some(), "the old version must now be closed");
    }

    /// A removed symbol's history must survive its own deletion — this is
    /// exactly why node_id is deliberately not a hard FK to nodes(id).
    #[test]
    fn removing_a_symbol_closes_its_version_history_without_deleting_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let symbol_id = store.nodes_by_kind(&repo, NodeKind::Symbol).unwrap().into_iter().find(|n| n.name.as_deref() == Some("greet")).unwrap().id;

        std::fs::remove_file(dir.path().join("main.py")).unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        assert!(store.get_node(&repo, symbol_id).unwrap().is_none(), "the node itself is really gone");
        let history = store.node_history(symbol_id).unwrap();
        assert_eq!(history.len(), 1, "history must survive the node's own deletion");
        assert!(history[0].valid_until.is_some(), "must be closed, not left open forever for a node that no longer exists");
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

    /// Regression test for a real gap found via live testing (the search
    /// page's kind=gotcha filter returned nothing): `ingest_vault` itself
    /// has no embedding step at all, unlike the symbol loop just above it
    /// in `persist()` -- without this, `with_embeddings: true` embedded
    /// every Symbol but silently left every Gotcha/Decision/Note node
    /// unembedded on every automatic rescan, forever.
    #[test]
    fn rescanning_with_embeddings_enabled_also_embeds_gotcha_decision_and_note_nodes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        let notes_dir = dir.path().join(".agentops").join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("token-bug.md"), "---\ntitle: \"Token bug\"\ntype: gotcha\n---\n\nverify_token has a known workaround for a bug.\n").unwrap();

        scan_and_persist(dir.path(), true).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let gotcha = store.nodes_by_kind(&repo, NodeKind::Gotcha).unwrap().into_iter().next().unwrap();
        let query_embedding = agentops_embeddings::LocalEmbedder.embed("a workaround for verify_token").unwrap();
        let hits = store.search_similar(&repo, &query_embedding, 5, Some(NodeKind::Gotcha)).unwrap();
        assert!(hits.iter().any(|(node, _)| node.id == gotcha.id), "the Gotcha node must be findable via semantic search after a rescan with embeddings enabled: {hits:?}");
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

    #[test]
    fn same_file_symbols_that_reference_each_other_get_reference_edges() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn helper() -> i32 { 1 }\n\nfn main() -> i32 { helper() }\n").unwrap();

        let summary = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary.reference_edges, 1);

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let symbols = store.nodes_by_kind(&repo, NodeKind::Symbol).unwrap();
        let main_fn = symbols.iter().find(|s| s.name.as_deref() == Some("main")).unwrap();
        let helper_fn = symbols.iter().find(|s| s.name.as_deref() == Some("helper")).unwrap();
        let edges = store.edges_from(&repo, main_fn.id).unwrap();
        assert!(edges.iter().any(|e| e.dst_id == helper_fn.id && e.relation == EdgeRelation::References));
    }

    #[test]
    fn a_symbol_name_mentioned_only_in_a_comment_gets_no_reference_edge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn helper() -> i32 { 1 }\n\n// does not call helper, just mentions it\nfn main() -> i32 { 2 }\n").unwrap();

        let summary = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary.reference_edges, 0, "a comment mentioning a sibling's name must never produce a reference edge");
    }

    #[test]
    fn reference_edges_are_reinforced_not_accumulated_on_rescan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn helper() -> i32 { 1 }\n\nfn main() -> i32 { helper() }\n").unwrap();

        let summary1 = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary1.reference_edges, 1);
        let summary2 = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary2.reference_edges, 1, "rescanning an unchanged file must not accumulate duplicate reference edges");

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        let references: Vec<_> = store.all_edges(&repo).unwrap().into_iter().filter(|e| e.relation == EdgeRelation::References).collect();
        assert_eq!(references.len(), 1, "still exactly one edge, not a duplicate -- upsert-and-reinforce, not accumulate");
        assert_eq!(references[0].weight, 1.5, "a reference re-confirmed on rescan must reinforce its existing edge's weight (Initiative 1), not reset it to a fresh 1.0");
    }

    #[test]
    fn a_reference_edge_is_pruned_once_its_target_reference_disappears() {
        let dir = tempfile::tempdir().unwrap();
        let main_rs = dir.path().join("main.rs");
        std::fs::write(&main_rs, "fn helper() -> i32 { 1 }\n\nfn main() -> i32 { helper() }\n").unwrap();
        scan_and_persist(dir.path(), false).unwrap();

        let store = open_store(dir.path());
        let repo = repo_name(dir.path());
        assert_eq!(store.all_edges(&repo).unwrap().into_iter().filter(|e| e.relation == EdgeRelation::References).count(), 1);
        drop(store);

        // `main` no longer calls `helper` -- the reference is gone.
        std::fs::write(&main_rs, "fn helper() -> i32 { 1 }\n\nfn main() -> i32 { 2 }\n").unwrap();
        let summary2 = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary2.reference_edges, 0, "the disappeared reference must not be counted");

        let store = open_store(dir.path());
        assert!(store.all_edges(&repo).unwrap().into_iter().filter(|e| e.relation == EdgeRelation::References).next().is_none(), "the stale edge must be pruned, not left behind decaying forever");
    }

    #[test]
    fn symbols_in_different_files_with_matching_names_never_get_a_reference_edge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn helper() -> i32 { 1 }\n\nfn main() -> i32 { 2 }\n").unwrap();
        std::fs::write(dir.path().join("src/b.rs"), "fn helper() -> i32 { 2 }\n").unwrap();

        let summary = scan_and_persist(dir.path(), false).unwrap();
        assert_eq!(summary.reference_edges, 0, "no call in either file's symbols references anything -- a same-named symbol in a different file must not spuriously match");
    }
}
