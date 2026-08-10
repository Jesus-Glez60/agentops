//! Shared "render this repo's onboarding doc" use case — used by both
//! `agentops-cli`'s `docgen` command and the `generate_docs` MCP tool.
//! Mirrors `init.rs`'s role for `init_agents_md`: an agent that only ever
//! talks to this system over MCP can generate docs for itself, without
//! needing shell access to the CLI.

use std::path::{Path, PathBuf};

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
    fn errors_clearly_when_repo_has_never_been_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let err = generate_docs(dir.path()).unwrap_err();
        assert!(err.to_string().contains("scan it first"), "{err}");
    }
}
