//! Install/scan state tracking (`~/.agentops/manifest.json`) — the registry
//! `agentops repos`/`agentops forget` read and mutate to answer "what's
//! indexed on this machine?" without already knowing a path to type in.
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

/// `pub` so a driving adapter that wants the real default path explicitly
/// (e.g. `agentops-api`'s `run()`, choosing between this and an env-var
/// override) doesn't have to duplicate the `$HOME`-joining logic.
pub fn default_manifest_path() -> PathBuf {
    agentops_data_dir().join("manifest.json")
}

/// `AGENTOPS_DATA_DIR` (default `~/.agentops`) — the single directory every
/// service's own per-subsystem `AGENTOPS_*_DB`/`*_DIR` var defaults under,
/// so a self-hosted deployment only needs to set one path to relocate all
/// of them at once; each individual var still overrides its own default.
fn agentops_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENTOPS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".agentops")
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

fn canonical_str(repo_path: &Path) -> String {
    repo_path.canonicalize().unwrap_or_else(|_| repo_path.to_path_buf()).to_string_lossy().to_string()
}

/// Records that `repo_path` was just (re)scanned — call this after a
/// successful `agentops install`. Canonicalizes the path first, so the same
/// repo scanned via a relative path and an absolute path doesn't create two
/// entries.
pub fn record_scan(repo_path: &Path) -> Result<()> {
    record_scan_at(&default_manifest_path(), repo_path)
}

/// `pub` for the same injectable-path reason as `list_scanned_repos_at`.
pub fn record_scan_at(manifest_file: &Path, repo_path: &Path) -> Result<()> {
    let canonical = canonical_str(repo_path);

    let mut manifest = load(manifest_file)?;
    match manifest.repos.iter_mut().find(|e| e.path == canonical) {
        Some(entry) => entry.last_scanned_at = now_unix(),
        None => manifest.repos.push(ManifestEntry { path: canonical, last_scanned_at: now_unix() }),
    }
    save(manifest_file, &manifest)
}

/// Lists every repo ever recorded via `record_scan`, most-recently-scanned first.
pub fn list_scanned_repos() -> Result<Vec<ManifestEntry>> {
    list_scanned_repos_at(&default_manifest_path())
}

/// `pub` so a driving adapter that needs an injectable manifest path (e.g.
/// `agentops-api`'s `GET /repos`, and its own tests) can list against a
/// specific file rather than always the real `$HOME/.agentops/manifest.json`
/// — avoids tests mutating shared global state or racing each other under
/// parallel test execution.
pub fn list_scanned_repos_at(manifest_file: &Path) -> Result<Vec<ManifestEntry>> {
    let mut manifest = load(manifest_file)?;
    manifest.repos.sort_by_key(|e| std::cmp::Reverse(e.last_scanned_at));
    Ok(manifest.repos)
}

/// Removes `repo_path`'s entry from the manifest. Returns `true` if an entry
/// was actually removed. Only ever touches the manifest — a repo's own
/// `.context/graph.db` is untouched, this is purely "stop listing it."
pub fn forget(repo_path: &Path) -> Result<bool> {
    forget_at(&default_manifest_path(), repo_path)
}

fn forget_at(manifest_file: &Path, repo_path: &Path) -> Result<bool> {
    let canonical = canonical_str(repo_path);
    let mut manifest = load(manifest_file)?;
    let before = manifest.repos.len();
    manifest.repos.retain(|e| e.path != canonical);
    let removed = manifest.repos.len() != before;
    save(manifest_file, &manifest)?;
    Ok(removed)
}

/// Removes every entry except `repo_path`. Returns the count removed.
pub fn forget_all_except(repo_path: &Path) -> Result<usize> {
    forget_all_except_at(&default_manifest_path(), repo_path)
}

fn forget_all_except_at(manifest_file: &Path, repo_path: &Path) -> Result<usize> {
    let canonical = canonical_str(repo_path);
    let mut manifest = load(manifest_file)?;
    let before = manifest.repos.len();
    manifest.repos.retain(|e| e.path == canonical);
    let removed = before - manifest.repos.len();
    save(manifest_file, &manifest)?;
    Ok(removed)
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

    #[test]
    fn forget_removes_exactly_the_matching_entry_and_no_others() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_file = dir.path().join("manifest.json");
        let repo_a = dir.path().join("repo-a");
        let repo_b = dir.path().join("repo-b");
        std::fs::create_dir_all(&repo_a).unwrap();
        std::fs::create_dir_all(&repo_b).unwrap();
        record_scan_at(&manifest_file, &repo_a).unwrap();
        record_scan_at(&manifest_file, &repo_b).unwrap();

        let removed = forget_at(&manifest_file, &repo_a).unwrap();
        assert!(removed);

        let repos = list_scanned_repos_at(&manifest_file).unwrap();
        assert_eq!(repos.len(), 1);
        assert!(repos[0].path.ends_with("repo-b"));
    }

    #[test]
    fn forgetting_an_unknown_repo_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_file = dir.path().join("manifest.json");
        let repo = dir.path().join("repo-a");
        std::fs::create_dir_all(&repo).unwrap();

        let removed = forget_at(&manifest_file, &repo).unwrap();
        assert!(!removed);
    }

    #[test]
    fn forget_all_except_keeps_only_the_named_repo() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_file = dir.path().join("manifest.json");
        let repo_a = dir.path().join("repo-a");
        let repo_b = dir.path().join("repo-b");
        let repo_c = dir.path().join("repo-c");
        std::fs::create_dir_all(&repo_a).unwrap();
        std::fs::create_dir_all(&repo_b).unwrap();
        std::fs::create_dir_all(&repo_c).unwrap();
        record_scan_at(&manifest_file, &repo_a).unwrap();
        record_scan_at(&manifest_file, &repo_b).unwrap();
        record_scan_at(&manifest_file, &repo_c).unwrap();

        let removed = forget_all_except_at(&manifest_file, &repo_b).unwrap();
        assert_eq!(removed, 2);

        let repos = list_scanned_repos_at(&manifest_file).unwrap();
        assert_eq!(repos.len(), 1);
        assert!(repos[0].path.ends_with("repo-b"));
    }
}
