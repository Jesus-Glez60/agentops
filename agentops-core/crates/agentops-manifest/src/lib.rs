//! Install/scan state tracking (`~/.agentops/manifest.json`) — the registry
//! the dashboard's repo-overview page reads to answer "what's indexed?"
//! without already knowing a path to type in.
//!
//! Deliberately just a flat JSON file, not a database: this tracks a small,
//! infrequently-written list (one entry per repo a developer has ever run
//! `agentops install` against on this machine), not query-able data — the
//! actual graph content lives in each repo's own `.context/graph.db`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    /// Unix seconds — kept as a plain number rather than pulling in a
    /// datetime crate just to format a timestamp; callers format for display.
    pub last_scanned_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    repos: Vec<ManifestEntry>,
}

fn default_manifest_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agentops").join("manifest.json")
}

fn load(manifest_file: &Path) -> Result<Manifest> {
    match std::fs::read_to_string(manifest_file) {
        Ok(contents) => serde_json::from_str(&contents).context("parsing manifest.json"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
        Err(e) => Err(e).context("reading manifest.json"),
    }
}

fn save(manifest_file: &Path, manifest: &Manifest) -> Result<()> {
    if let Some(parent) = manifest_file.parent() {
        std::fs::create_dir_all(parent).context("creating ~/.agentops")?;
    }
    std::fs::write(manifest_file, serde_json::to_string_pretty(manifest)?).context("writing manifest.json")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Records that `repo_path` was just (re)scanned — call this after a
/// successful `agentops install`. Canonicalizes the path first, so the same
/// repo scanned via a relative path and an absolute path doesn't create two
/// entries.
pub fn record_scan(repo_path: &Path) -> Result<()> {
    record_scan_at(&default_manifest_path(), repo_path)
}

fn record_scan_at(manifest_file: &Path, repo_path: &Path) -> Result<()> {
    let canonical = repo_path.canonicalize().unwrap_or_else(|_| repo_path.to_path_buf());
    let canonical_str = canonical.to_string_lossy().to_string();

    let mut manifest = load(manifest_file)?;
    match manifest.repos.iter_mut().find(|e| e.path == canonical_str) {
        Some(entry) => entry.last_scanned_at = now_unix(),
        None => manifest.repos.push(ManifestEntry { path: canonical_str, last_scanned_at: now_unix() }),
    }
    save(manifest_file, &manifest)
}

/// Lists every repo ever recorded via `record_scan`, most-recently-scanned first.
pub fn list_scanned_repos() -> Result<Vec<ManifestEntry>> {
    list_scanned_repos_at(&default_manifest_path())
}

fn list_scanned_repos_at(manifest_file: &Path) -> Result<Vec<ManifestEntry>> {
    let mut manifest = load(manifest_file)?;
    manifest.repos.sort_by(|a, b| b.last_scanned_at.cmp(&a.last_scanned_at));
    Ok(manifest.repos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_a_new_repo_adds_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_file = dir.path().join("manifest.json");
        let repo = dir.path().join("repo-a");
        std::fs::create_dir_all(&repo).unwrap();

        record_scan_at(&manifest_file, &repo).unwrap();
        let repos = list_scanned_repos_at(&manifest_file).unwrap();
        assert_eq!(repos.len(), 1);
        assert!(repos[0].path.ends_with("repo-a"));
    }

    #[test]
    fn rescanning_the_same_repo_updates_instead_of_duplicating() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_file = dir.path().join("manifest.json");
        let repo = dir.path().join("repo-a");
        std::fs::create_dir_all(&repo).unwrap();

        record_scan_at(&manifest_file, &repo).unwrap();
        record_scan_at(&manifest_file, &repo).unwrap();
        let repos = list_scanned_repos_at(&manifest_file).unwrap();
        assert_eq!(repos.len(), 1, "rescanning the same repo must update its entry, not duplicate it");
    }

    #[test]
    fn most_recently_scanned_repo_is_listed_first() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_file = dir.path().join("manifest.json");
        let repo_a = dir.path().join("repo-a");
        let repo_b = dir.path().join("repo-b");
        std::fs::create_dir_all(&repo_a).unwrap();
        std::fs::create_dir_all(&repo_b).unwrap();

        record_scan_at(&manifest_file, &repo_a).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        record_scan_at(&manifest_file, &repo_b).unwrap();

        let repos = list_scanned_repos_at(&manifest_file).unwrap();
        assert!(repos[0].path.ends_with("repo-b"), "{repos:?}");
    }

    #[test]
    fn listing_with_no_manifest_file_yet_returns_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_file = dir.path().join("does-not-exist.json");
        let repos = list_scanned_repos_at(&manifest_file).unwrap();
        assert!(repos.is_empty());
    }
}
