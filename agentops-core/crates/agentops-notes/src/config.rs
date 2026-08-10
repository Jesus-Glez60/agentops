//! Resolves where a repo's notes live — the actual fix for the write-back
//! protocol found hardcoded to one personal vault path
//! (`~/Vaults/dev-knowledge-base`) with no connection to what `AGENTS.md`
//! generation emitted. Resolution order: an explicit override → a
//! `.agentops/config.json` in the repo if one sets `notes_path` → a default
//! in-repo `.agentops/notes/` folder that always works with zero setup —
//! the point being that ingestion and write-back (an `add_note` MCP tool,
//! `agentops-agents-md`'s emitted `NOTES_PATH`, the installed prompt pack's
//! write-back instructions) all resolve to the exact same location, for a
//! user who has no organized vault at all as much as for one who does.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
struct NotesConfig {
    notes_path: Option<String>,
}

/// The zero-setup fallback: a folder inside the repo itself, created on
/// first use by `add_note`/`ingest_notes` if it doesn't exist yet.
pub fn default_notes_path(repo_path: &Path) -> PathBuf {
    repo_path.join(".agentops").join("notes")
}

/// Resolves the effective notes path for `repo_path`. `explicit` (e.g. a
/// caller-supplied `notes_path` argument) always wins; otherwise reads
/// `.agentops/config.json`'s `notes_path` if present (relative paths are
/// resolved against `repo_path`, so an external vault like
/// `~/Vaults/my-project` can be configured without hardcoding it into any
/// generated file); otherwise falls back to `default_notes_path`.
pub fn resolve_notes_path(repo_path: &Path, explicit: Option<&Path>) -> PathBuf {
    if let Some(explicit) = explicit {
        return explicit.to_path_buf();
    }

    if let Some(configured) = read_config(repo_path).notes_path {
        let configured_path = PathBuf::from(&configured);
        return if configured_path.is_absolute() { configured_path } else { repo_path.join(configured_path) };
    }

    default_notes_path(repo_path)
}

fn read_config(repo_path: &Path) -> NotesConfig {
    let config_path = repo_path.join(".agentops").join("config.json");
    std::fs::read_to_string(config_path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_override_always_wins() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = Path::new("/some/external/vault");
        assert_eq!(resolve_notes_path(dir.path(), Some(explicit)), explicit);
    }

    #[test]
    fn falls_back_to_in_repo_default_with_zero_configuration() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(resolve_notes_path(dir.path(), None), default_notes_path(dir.path()));
    }

    #[test]
    fn reads_an_absolute_notes_path_from_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".agentops")).unwrap();
        std::fs::write(dir.path().join(".agentops/config.json"), r#"{"notes_path": "/home/user/Vaults/my-project"}"#).unwrap();

        assert_eq!(resolve_notes_path(dir.path(), None), PathBuf::from("/home/user/Vaults/my-project"));
    }

    #[test]
    fn resolves_a_relative_configured_notes_path_against_the_repo_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".agentops")).unwrap();
        std::fs::write(dir.path().join(".agentops/config.json"), r#"{"notes_path": "docs/knowledge"}"#).unwrap();

        assert_eq!(resolve_notes_path(dir.path(), None), dir.path().join("docs/knowledge"));
    }

    #[test]
    fn a_missing_or_malformed_config_file_never_fails_resolution() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".agentops")).unwrap();
        std::fs::write(dir.path().join(".agentops/config.json"), "not valid json").unwrap();

        assert_eq!(resolve_notes_path(dir.path(), None), default_notes_path(dir.path()));
    }
}
