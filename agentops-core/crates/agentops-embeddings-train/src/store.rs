//! Versioned, per-repo storage for trained projection heads (Initiative 5,
//! CLS-inspired retrieval plan) -- `safetensors` checkpoints plus a plain
//! "active" pointer file, the same shape this codebase already uses for
//! other local, file-based state (`~/.agentops/manifest.json`).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use candle_core::{DType, Device};
use candle_nn::{VarBuilder, VarMap};

use crate::model::ProjectionHead;
use crate::replay::ReplayPair;

pub struct ProjectionStore {
    dir: PathBuf,
}

impl ProjectionStore {
    /// `~/.agentops/models/{repo}` -- `repo` is already a filesystem-safe
    /// canonicalized directory name by the time it reaches here (the same
    /// `agentops_mcp::scan::repo_name()` convention every other per-repo
    /// path in this codebase already relies on), so no further sanitizing
    /// is done here.
    pub fn open(repo: &str) -> Result<Self> {
        let base = dirs_home().context("resolving home directory for ~/.agentops/models")?;
        let dir = base.join(".agentops").join("models").join(repo);
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Test-only constructor pointing at an arbitrary directory, so tests
    /// don't touch the real `~/.agentops`.
    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn version_path(&self, version: u32) -> PathBuf {
        self.dir.join(format!("proj_head_v{version}.safetensors"))
    }

    fn active_pointer_path(&self) -> PathBuf {
        self.dir.join("active_version")
    }

    fn anchor_path(&self) -> PathBuf {
        self.dir.join("anchor_pairs.json")
    }

    /// The next version number to write -- one past whatever's already on
    /// disk, starting at 1.
    fn next_version(&self) -> Result<u32> {
        let mut max = 0u32;
        for entry in fs::read_dir(&self.dir)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix("proj_head_v").and_then(|s| s.strip_suffix(".safetensors")) {
                if let Ok(v) = rest.parse::<u32>() {
                    max = max.max(v);
                }
            }
        }
        Ok(max + 1)
    }

    /// Saves `varmap`'s trained weights as a new, unpromoted version.
    /// Returns the version number -- callers decide whether to `promote`
    /// it based on eval results; a saved-but-unpromoted version is
    /// harmless disk usage, not a live state change.
    pub fn save_new_version(&self, varmap: &VarMap) -> Result<u32> {
        let version = self.next_version()?;
        varmap.save(self.version_path(version))?;
        Ok(version)
    }

    /// Makes `version` the one `load_active` returns from now on.
    pub fn promote(&self, version: u32) -> Result<()> {
        fs::write(self.active_pointer_path(), version.to_string())?;
        Ok(())
    }

    pub fn active_version(&self) -> Result<Option<u32>> {
        match fs::read_to_string(self.active_pointer_path()) {
            Ok(s) => Ok(s.trim().parse().ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Loads the currently-active projection head, if any repo has been
    /// consolidated and promoted yet. `None` (not an error) for a
    /// never-consolidated repo -- query-time callers must treat this as
    /// "use the raw base embedding," the same graceful-degradation
    /// convention `Node.last_touched_at: None` already established for
    /// Initiative 3.
    pub fn load_active(&self, device: &Device) -> Result<Option<ProjectionHead>> {
        let Some(version) = self.active_version()? else { return Ok(None) };
        self.load_version(version, device)
    }

    fn load_version(&self, version: u32, device: &Device) -> Result<Option<ProjectionHead>> {
        let path = self.version_path(version);
        if !path.exists() {
            return Ok(None);
        }
        // Standard candle load idiom: build the model fresh (registering
        // its parameter names/shapes in a new VarMap), then load the
        // checkpoint into that same VarMap, which overwrites the freshly
        // -initialized values with the saved ones.
        let mut varmap = VarMap::new();
        let vb = VarBuilder::from_varmap(&varmap, DType::F32, device);
        let head = ProjectionHead::new(vb)?;
        varmap.load(&path)?;
        Ok(Some(head))
    }

    pub fn load_anchor(&self) -> Result<Vec<ReplayPair>> {
        match fs::read_to_string(self.anchor_path()) {
            Ok(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_anchor(&self, pairs: &[ReplayPair]) -> Result<()> {
        fs::write(self.anchor_path(), serde_json::to_string(pairs)?)?;
        Ok(())
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_new_version_starts_at_one_and_increments() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProjectionStore::at(tmp.path().to_path_buf()).unwrap();
        let (varmap, _head) = fresh_head();

        assert_eq!(store.save_new_version(&varmap).unwrap(), 1);
        assert_eq!(store.save_new_version(&varmap).unwrap(), 2);
    }

    #[test]
    fn active_version_is_none_until_promoted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProjectionStore::at(tmp.path().to_path_buf()).unwrap();
        let (varmap, _head) = fresh_head();
        let v = store.save_new_version(&varmap).unwrap();

        assert_eq!(store.active_version().unwrap(), None);
        store.promote(v).unwrap();
        assert_eq!(store.active_version().unwrap(), Some(v));
    }

    #[test]
    fn load_active_round_trips_a_promoted_version() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProjectionStore::at(tmp.path().to_path_buf()).unwrap();
        let (varmap, _head) = fresh_head();
        let v = store.save_new_version(&varmap).unwrap();
        store.promote(v).unwrap();

        let loaded = store.load_active(&Device::Cpu).unwrap();
        assert!(loaded.is_some());
    }

    #[test]
    fn load_active_is_none_for_a_never_consolidated_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProjectionStore::at(tmp.path().to_path_buf()).unwrap();
        assert!(store.load_active(&Device::Cpu).unwrap().is_none());
    }

    #[test]
    fn anchor_pairs_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProjectionStore::at(tmp.path().to_path_buf()).unwrap();
        assert!(store.load_anchor().unwrap().is_empty());

        let pairs = vec![ReplayPair { anchor_id: 1, positive_id: 2, weight: 3.5 }];
        store.save_anchor(&pairs).unwrap();
        let loaded = store.load_anchor().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].weight, 3.5);
    }

    fn fresh_head() -> (VarMap, ProjectionHead) {
        let varmap = VarMap::new();
        let vb = VarBuilder::from_varmap(&varmap, DType::F32, &Device::Cpu);
        let head = ProjectionHead::new(vb).unwrap();
        (varmap, head)
    }
}
