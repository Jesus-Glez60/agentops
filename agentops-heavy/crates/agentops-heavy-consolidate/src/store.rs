//! Versioned, per-repo storage for trained LoRA adapters (Initiative 6,
//! CLS-inspired retrieval plan) -- deliberately mirrors
//! `agentops-embeddings-train::ProjectionStore`'s exact shape (versioned
//! `safetensors` + a plain "active" pointer file under
//! `~/.agentops/models/{repo}/`), just with its own filename prefix so the
//! two initiatives' artifacts coexist in the same per-repo directory
//! without colliding.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use candle_nn::VarMap;

pub struct AdapterStore {
    dir: PathBuf,
}

impl AdapterStore {
    pub fn open(repo: &str) -> Result<Self> {
        let base = std::env::var_os("HOME").map(PathBuf::from).context("resolving home directory for ~/.agentops/models")?;
        let dir = base.join(".agentops").join("models").join(repo);
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn version_path(&self, version: u32) -> PathBuf {
        self.dir.join(format!("lora_adapter_v{version}.safetensors"))
    }

    fn active_pointer_path(&self) -> PathBuf {
        self.dir.join("active_model_version")
    }

    fn next_version(&self) -> Result<u32> {
        let mut max = 0u32;
        for entry in fs::read_dir(&self.dir)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix("lora_adapter_v").and_then(|s| s.strip_suffix(".safetensors")) {
                if let Ok(v) = rest.parse::<u32>() {
                    max = max.max(v);
                }
            }
        }
        Ok(max + 1)
    }

    pub fn save_new_version(&self, varmap: &VarMap) -> Result<u32> {
        let version = self.next_version()?;
        varmap.save(self.version_path(version))?;
        Ok(version)
    }

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

    /// Loads version `version`'s weights *into* `varmap` -- the caller
    /// already owns a `VarMap` wired up to a live `AdaptedModel` (built via
    /// `AdaptedModel::load`, whose adapter starts as a fresh, zero
    /// -initialized no-op), and this overwrites those values in place,
    /// same load idiom `agentops-embeddings-train::ProjectionStore::
    /// load_version` already uses.
    pub fn load_version_into(&self, version: u32, varmap: &mut VarMap) -> Result<bool> {
        let path = self.version_path(version);
        if !path.exists() {
            return Ok(false);
        }
        varmap.load(&path)?;
        Ok(true)
    }

    pub fn load_active_into(&self, varmap: &mut VarMap) -> Result<bool> {
        let Some(version) = self.active_version()? else { return Ok(false) };
        self.load_version_into(version, varmap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{DType, Device};
    use candle_nn::VarBuilder;

    fn fresh_varmap() -> VarMap {
        let varmap = VarMap::new();
        let vb = VarBuilder::from_varmap(&varmap, DType::F32, &Device::Cpu);
        let _ = vb.get_with_hints(4, "dummy", candle_nn::init::ZERO).unwrap();
        varmap
    }

    #[test]
    fn save_new_version_starts_at_one_and_increments() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AdapterStore::at(tmp.path().to_path_buf()).unwrap();
        let varmap = fresh_varmap();

        assert_eq!(store.save_new_version(&varmap).unwrap(), 1);
        assert_eq!(store.save_new_version(&varmap).unwrap(), 2);
    }

    #[test]
    fn active_version_is_none_until_promoted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AdapterStore::at(tmp.path().to_path_buf()).unwrap();
        let varmap = fresh_varmap();
        let v = store.save_new_version(&varmap).unwrap();

        assert_eq!(store.active_version().unwrap(), None);
        store.promote(v).unwrap();
        assert_eq!(store.active_version().unwrap(), Some(v));
    }

    #[test]
    fn load_active_into_round_trips_a_promoted_version() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AdapterStore::at(tmp.path().to_path_buf()).unwrap();
        let varmap = fresh_varmap();
        let v = store.save_new_version(&varmap).unwrap();
        store.promote(v).unwrap();

        let mut loaded_varmap = fresh_varmap();
        assert!(store.load_active_into(&mut loaded_varmap).unwrap());
    }

    #[test]
    fn load_active_into_is_false_for_a_never_consolidated_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AdapterStore::at(tmp.path().to_path_buf()).unwrap();
        let mut varmap = fresh_varmap();
        assert!(!store.load_active_into(&mut varmap).unwrap());
    }
}
