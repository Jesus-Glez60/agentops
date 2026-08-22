//! Shared "render this repo's onboarding doc" use case — used by both
//! `agentops-cli`'s `docgen` command and the `generate_docs` MCP tool.
//! Mirrors `init.rs`'s role for `init_agents_md`: an agent that only ever
//! talks to this system over MCP can generate docs for itself, without
//! needing shell access to the CLI.

use std::path::{Path, PathBuf};

use agentops_embeddings::Embedder;
use agentops_graph::{upsert_node, EdgeRelation, GraphStore, NewNode, NodeKind};
use agentops_scanner::ScannedFile;
use anyhow::{Context, Result};

use crate::scan::repo_name;

/// Re-scans `repo_path` read-only purely to recompute the PageRank file
/// ordering (ranking isn't persisted anywhere — it's cheap to recompute and
/// `agentops-docgen` deliberately doesn't own scanning, see the plan's
/// crate-boundary design), then renders and writes `repo-map.md`. Errors
/// clearly if the repo has never been scanned at all, rather than silently
/// producing a doc with zero files/symbols.
pub fn generate_docs(repo_path: &Path) -> Result<PathBuf> {
    let store = crate::store::open_store(repo_path)?;
    let repo = repo_name(repo_path);
    // Backend-agnostic "has this repo ever been scanned" check — a SQLite
    // file's existence (the old check) doesn't mean anything once
    // AGENTOPS_DATABASE_URL can select Postgres instead.
    if store.latest_scan(&repo)?.is_none() {
        anyhow::bail!("no scans recorded for this repo yet — scan it first (agentops install / the scan_repo tool)");
    }

    let report = agentops_scanner::scan_repo(repo_path).context("scanning repo for ranking")?;
    let ranked: Vec<PathBuf> = agentops_scanner::rank_files(repo_path, &report.files).into_iter().map(|(p, _)| p).collect();

    let doc = agentops_docgen::render_onboarding_doc(store.as_ref(), &repo, &ranked)?;
    let out_path = repo_path.join("repo-map.md");
    agentops_docgen::write_to_file(&doc, &out_path)?;

    Ok(out_path)
}

/// Builds and persists `repo`'s Documentation Viewer page — the
/// orchestration step neither `agentops-docgen` (LLM-free by design) nor
/// `agentops-llm` (scan/rank-free by design) can own on its own, since it's
/// the one crate that already depends on both plus `agentops-graph`'s
/// store. Called from `scan::persist` right after `refresh_repo_state`,
/// reusing `report.files` (already scanned this call) rather than
/// re-scanning.
///
/// The LLM-assisted module-labeling step is itself best-effort: if
/// `AGENTOPS_ANTHROPIC_API_KEY` isn't set, or the API call fails, or the
/// response doesn't parse, this falls back to an empty label set, which
/// `agentops_docgen::build_doc_page` turns into its own internal
/// directory-name heuristic grouping. Only a genuine failure to *build or
/// persist* the doc page at all propagates to the caller (which itself
/// only logs it — see `scan::persist`'s call site).
///
/// Also indexes each section as its own searchable `NodeKind::DocSection`
/// node (`index_doc_sections`, Initiative 2, CLS-inspired retrieval plan) —
/// the "gist" tier `search_gist_then_detail` queries, previously write-only
/// and invisible to search. `with_embeddings` gates this the same way
/// `scan::persist` already gates Symbol/Gotcha/Decision/Note embedding —
/// opt-in real CPU-bound cost, not something every scan should pay.
pub fn persist_doc_page(store: &dyn GraphStore, repo_path: &Path, repo: &str, files: &[ScannedFile], with_embeddings: bool) -> Result<()> {
    let ranked: Vec<PathBuf> = agentops_scanner::rank_files(repo_path, files).into_iter().map(|(p, _)| p).collect();

    let module_labels = agentops_llm::AnthropicConfig::from_env()
        .and_then(|config| agentops_llm::group_core_modules(&config, repo, &ranked))
        .unwrap_or_default();

    let doc_page = agentops_docgen::build_doc_page(store, repo, &ranked, &module_labels)?;
    index_doc_sections(store, repo, &doc_page, with_embeddings)?;
    let content_json = serde_json::to_string(&doc_page).context("serializing the generated doc page")?;
    store.save_doc_page(repo, &doc_page.generated_at, &content_json)?;
    Ok(())
}

/// Upserts one `NodeKind::DocSection` node per `doc_page.sections`, natural-
/// keyed on `path: "doc_section:{section.id}"` (stable across regenerations
/// as long as the section's own slug doesn't change — same pseudo-path
/// idiom `agentops-notes` uses for vault notes, `"vault:{source_path}"`).
/// `Covers` edges to this section are fully replaced each call (like
/// `DependsOn`, not reinforced like `References`/`Affects`) since a
/// section's coverage is a deterministic function of the current doc build,
/// not something a human/agent action re-confirms.
fn index_doc_sections(store: &dyn GraphStore, repo: &str, doc_page: &agentops_docgen::DocPage, with_embeddings: bool) -> Result<()> {
    for section in &doc_page.sections {
        let (text, covered_ids) = section.search_text_and_covered_ids();
        let node_id = upsert_node(
            store,
            NewNode {
                kind: NodeKind::DocSection,
                repo: repo.to_string(),
                path: Some(format!("doc_section:{}", section.id)),
                name: Some(section.title.clone()),
                container: None,
                start_line: None,
                end_line: None,
                content: Some(text.clone()),
            },
        )?;

        store.delete_edges_from(repo, node_id, EdgeRelation::Covers)?;
        for covered_id in covered_ids {
            store.add_edge(repo, node_id, covered_id, EdgeRelation::Covers)?;
        }

        if with_embeddings && !text.trim().is_empty() {
            let embedding = agentops_embeddings::LocalEmbedder.embed(&text)?;
            store.set_embedding(repo, node_id, &embedding)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_a_doc_after_a_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let out_path = generate_docs(dir.path()).unwrap();
        let content = std::fs::read_to_string(&out_path).unwrap();
        assert!(content.contains("Symbols indexed: 1"));
        assert!(content.contains("greet"));
    }

    #[test]
    fn a_scan_persists_a_doc_page_reachable_via_get_doc_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());
        let (_generated_at, content_json) = store.get_doc_page(&repo).unwrap().expect("scan_and_persist must have persisted a doc page");
        let parsed: serde_json::Value = serde_json::from_str(&content_json).unwrap();
        assert_eq!(parsed["repo"], repo);
        // No AGENTOPS_ANTHROPIC_API_KEY in the test environment -- must fall
        // back to the directory heuristic (no `src/` dir here, so Core
        // Modules is empty) rather than failing the scan.
        assert!(parsed["sections"].as_array().unwrap().iter().any(|s| s["id"] == "overview"));
    }

    /// Initiative 2 (CLS-inspired retrieval plan): a scan must index the
    /// generated doc page's sections as their own searchable
    /// `NodeKind::DocSection` nodes -- previously docgen's output was
    /// write-only, invisible to search entirely.
    #[test]
    fn a_scan_indexes_doc_sections_as_searchable_nodes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());
        let sections = store.nodes_by_kind(&repo, agentops_graph::NodeKind::DocSection).unwrap();
        assert!(!sections.is_empty(), "at least the overview section must have been indexed");
        let overview = sections.iter().find(|n| n.path.as_deref() == Some("doc_section:overview")).expect("overview section must be indexed under its stable pseudo-path");
        assert!(overview.content.is_some(), "the section's flattened block text must be stored as its content");
    }

    /// `Covers` edges (not `Documents`) connect a `DocSection` to the
    /// symbols it covers, feeding `search_gist_then_detail`'s second-pass
    /// scoping.
    #[test]
    fn indexed_doc_sections_cover_the_symbols_their_symbol_table_lists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/greeting")).unwrap();
        std::fs::write(dir.path().join("src/greeting/greet.py"), "def greet():\n    return 'hi'\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());
        let greet = store.find_node(&repo, agentops_graph::NodeKind::Symbol, Some("src/greeting/greet.py"), Some("greet"), None).unwrap().expect("greet must have been scanned");

        let sections = store.nodes_by_kind(&repo, agentops_graph::NodeKind::DocSection).unwrap();
        let covering_section = sections.iter().find(|s| store.edges_from(&repo, s.id).unwrap().iter().any(|e| e.relation == EdgeRelation::Covers && e.dst_id == greet.id));
        assert!(covering_section.is_some(), "some section's SymbolTable must produce a Covers edge to the greet symbol: sections={sections:?}");
    }

    #[test]
    fn doc_sections_are_only_embedded_when_with_embeddings_is_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());
        let query = agentops_embeddings::LocalEmbedder.embed("repository overview").unwrap();
        let hits = store.search_similar(&repo, &query, 10, Some(agentops_graph::NodeKind::DocSection)).unwrap();
        assert!(hits.is_empty(), "without with_embeddings, no DocSection should be embedded/findable via search_similar");
        drop(store);

        crate::scan::scan_and_persist(dir.path(), true).unwrap();
        let store = crate::store::open_store(dir.path()).unwrap();
        let hits = store.search_similar(&repo, &query, 10, Some(agentops_graph::NodeKind::DocSection)).unwrap();
        assert!(!hits.is_empty(), "with with_embeddings, the overview section must be embedded and findable");
    }

    #[test]
    fn errors_clearly_when_repo_has_never_been_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let err = generate_docs(dir.path()).unwrap_err();
        assert!(err.to_string().contains("scan it first"), "{err}");
    }
}
