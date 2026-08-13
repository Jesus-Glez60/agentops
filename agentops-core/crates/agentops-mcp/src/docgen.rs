//! Shared "render this repo's onboarding doc" use case — used by both
//! `agentops-cli`'s `docgen` command and the `generate_docs` MCP tool.
//! Mirrors `init.rs`'s role for `init_agents_md`: an agent that only ever
//! talks to this system over MCP can generate docs for itself, without
//! needing shell access to the CLI.

use std::path::{Path, PathBuf};

use agentops_graph::GraphStore;
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
pub fn persist_doc_page(store: &dyn GraphStore, repo_path: &Path, repo: &str, files: &[ScannedFile]) -> Result<()> {
    let ranked: Vec<PathBuf> = agentops_scanner::rank_files(repo_path, files).into_iter().map(|(p, _)| p).collect();

    let module_labels = agentops_llm::AnthropicConfig::from_env()
        .and_then(|config| agentops_llm::group_core_modules(&config, repo, &ranked))
        .unwrap_or_default();

    let doc_page = agentops_docgen::build_doc_page(store, repo, &ranked, &module_labels)?;
    let content_json = serde_json::to_string(&doc_page).context("serializing the generated doc page")?;
    store.save_doc_page(repo, &doc_page.generated_at, &content_json)?;
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

    #[test]
    fn errors_clearly_when_repo_has_never_been_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let err = generate_docs(dir.path()).unwrap_err();
        assert!(err.to_string().contains("scan it first"), "{err}");
    }
}
