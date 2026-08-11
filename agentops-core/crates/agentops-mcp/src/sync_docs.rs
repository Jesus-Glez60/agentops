//! The automatic (non-interactive) two-thirds of docbrain's library
//! discovery — registry metadata, then GitHub search. Factored out of
//! `agentops-cli`'s `sync_docs` (which owns the third, inherently-CLI
//! step 3: asking the human interactively) so callers with no terminal
//! at all — the Linear webhook auto-kickoff dispatch path — can still run
//! discovery, exactly like `scan_and_persist` was already shared for the
//! same "agent-callable, not just human-callable" reason.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use docbrain_ingest::{classify_dependency, LibraryDiscovery, RegistryDiscovery};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncDocsSummary {
    pub already_known: u32,
    pub discovered: u32,
    pub unresolved: Vec<String>,
}

/// `~/.agentops/docbrain.db` — duplicated (not depended-on) from
/// `docbrain_mcp::default_db_path`: pulling in `docbrain-mcp` here just for
/// one path constant would be a heavier cross-crate wire than this one-line
/// join warrants.
fn default_docbrain_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agentops").join("docbrain.db")
}

/// Scans `repo_path` for third-party dependencies and registers whichever
/// ones aren't already known in the docbrain store, via registry metadata
/// then GitHub search — no interactive prompt, so this is safe to call from
/// any non-terminal context (a webhook dispatch, an MCP tool call).
pub fn sync_docs(repo_path: &Path, docbrain_db_path: Option<&Path>) -> Result<SyncDocsSummary> {
    let report = agentops_scanner::scan_repo(repo_path).context("scanning repo for dependencies")?;

    let mut candidates: BTreeSet<(docbrain_ingest::Ecosystem, String)> = BTreeSet::new();
    for file in &report.files {
        let language = file.language.tree_sitter_name();
        for dep in &file.deps {
            if let Some(pair) = classify_dependency(language, dep) {
                candidates.insert(pair);
            }
        }
    }

    let db_path = docbrain_db_path.map(Path::to_path_buf).unwrap_or_else(default_docbrain_db_path);
    let store = docbrain_graph::SqliteDocbrainStore::open(&db_path).context("opening docbrain store")?;

    sync_candidates(&store, &RegistryDiscovery, &candidates)
}

/// The discovery loop on its own, for testability against a fake
/// `LibraryDiscovery` without hitting the real npm/PyPI/GitHub network.
fn sync_candidates(store: &dyn docbrain_graph::DocbrainStore, discovery: &dyn LibraryDiscovery, candidates: &BTreeSet<(docbrain_ingest::Ecosystem, String)>) -> Result<SyncDocsSummary> {
    let mut summary = SyncDocsSummary::default();

    for (ecosystem, name) in candidates {
        if store.get_library(name)?.is_some() {
            summary.already_known += 1;
            continue;
        }

        let step1 = discovery.discover(*ecosystem, name).unwrap_or(None);
        if let Some(found) = step1 {
            store.add_library(name, name, found.repo_url.as_deref(), found.docs_url.as_deref())?;
            summary.discovered += 1;
            continue;
        }

        let step2 = discovery.search_github(name).unwrap_or(None);
        if let Some(found) = step2 {
            store.add_library(name, name, found.repo_url.as_deref(), found.docs_url.as_deref())?;
            summary.discovered += 1;
            continue;
        }

        summary.unresolved.push(name.clone());
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use docbrain_ingest::{DiscoveredLibrary, Ecosystem};

    struct FakeDiscovery {
        known: Vec<&'static str>,
        github_only: Vec<&'static str>,
    }

    impl LibraryDiscovery for FakeDiscovery {
        fn discover(&self, _ecosystem: Ecosystem, name: &str) -> Result<Option<DiscoveredLibrary>> {
            Ok(self.known.contains(&name).then(|| DiscoveredLibrary { name: name.to_string(), description: None, docs_url: Some(format!("https://registry.example/{name}")), repo_url: None }))
        }

        fn search_github(&self, name: &str) -> Result<Option<DiscoveredLibrary>> {
            Ok(self.github_only.contains(&name).then(|| DiscoveredLibrary { name: name.to_string(), description: None, docs_url: None, repo_url: Some(format!("https://github.com/example/{name}")) }))
        }
    }

    #[test]
    fn discovery_order_prefers_registry_then_github_then_leaves_the_rest_unresolved() {
        let store = docbrain_graph::SqliteDocbrainStore::open_in_memory().unwrap();
        let discovery = FakeDiscovery { known: vec!["next"], github_only: vec!["some-tool"] };
        let candidates = BTreeSet::from([(Ecosystem::Npm, "next".to_string()), (Ecosystem::Npm, "some-tool".to_string()), (Ecosystem::Npm, "totally-unknown".to_string())]);

        let summary = sync_candidates(&store, &discovery, &candidates).unwrap();

        assert_eq!(summary.already_known, 0);
        assert_eq!(summary.discovered, 2, "both the registry hit and the GitHub-only hit count as discovered");
        assert_eq!(summary.unresolved, vec!["totally-unknown".to_string()]);

        use docbrain_graph::DocbrainStore;
        assert!(store.get_library("next").unwrap().is_some());
        assert!(store.get_library("some-tool").unwrap().is_some());
        assert!(store.get_library("totally-unknown").unwrap().is_none());
    }

    #[test]
    fn a_library_already_known_is_not_rediscovered() {
        let store = docbrain_graph::SqliteDocbrainStore::open_in_memory().unwrap();
        use docbrain_graph::DocbrainStore;
        store.add_library("next", "Next.js", None, Some("https://nextjs.org")).unwrap();

        let discovery = FakeDiscovery { known: vec!["next"], github_only: vec![] };
        let candidates = BTreeSet::from([(Ecosystem::Npm, "next".to_string())]);

        let summary = sync_candidates(&store, &discovery, &candidates).unwrap();

        assert_eq!(summary.already_known, 1);
        assert_eq!(summary.discovered, 0, "already-known libraries must not be re-queried against discovery");
    }
}
