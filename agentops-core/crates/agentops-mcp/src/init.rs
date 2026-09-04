//! Shared "bootstrap AGENTS.md/NOTES_PATH for this repo" use case — used by
//! both `agentops-cli`'s `install` command and the `init_agents_md` MCP
//! tool. Exists specifically so an agent that only ever talks to this
//! system over MCP (the "prompts protocol first" path — see
//! `agentops-core/prompts/docs/vault-protocol.md`'s rule 1: always read
//! `AGENTS.md` before any vault write) can bootstrap it for itself, without
//! needing shell access to the CLI.

use std::path::{Path, PathBuf};

use anyhow::Result;

pub struct InitResult {
    pub agents_md_path: PathBuf,
    pub notes_path: PathBuf,
}

/// If `notes_path_override` is given, persists it to `.agentops/config.json`
/// first (so every future `resolve_notes_path` call — CLI, MCP, REST —
/// agrees on it without re-passing the flag), then (re)writes `AGENTS.md`
/// with the resolved `NOTES_PATH` and ensures `.gitignore` excludes
/// generated scan output. Safe to call repeatedly — always just refreshes.
pub fn init_agents_md(repo_path: &Path, notes_path_override: Option<&Path>) -> Result<InitResult> {
    if let Some(np) = notes_path_override {
        agentops_notes::write_notes_path(repo_path, &np.display().to_string())?;
    }
    let notes_path = agentops_notes::resolve_notes_path(repo_path, None);

    let opts = agentops_agents_md::GenerateOptions { notes_path: Some(notes_path.display().to_string()), repo_map_path: None, claude_code_installed: false };
    let content = agentops_agents_md::generate(repo_path, &opts);
    let agents_md_path = repo_path.join("AGENTS.md");
    std::fs::write(&agents_md_path, &content)?;

    ensure_gitignore_entries(repo_path)?;

    Ok(InitResult { agents_md_path, notes_path })
}

/// Adds `.context/` to the target repo's `.gitignore` if not already
/// present — generated scan output is a structured map of the whole
/// codebase's internals and shouldn't be committed by default. Ported from
/// `main`. `.agentops/notes/` is deliberately **not** added here: unlike
/// scan output, notes are real, potentially team-worth-keeping knowledge,
/// not a regenerable cache.
///
/// `pub`, not just called from `init_agents_md`: `.context/agentops-
/// remote.json` (a live API key, written by `agentops-cli`'s `connect
/// --remote`) also lives under `.context/`, and `connect_remote` skips
/// `init_agents_md` entirely whenever `AGENTS.md` already exists (the
/// common re-run case) -- calling this directly and unconditionally from
/// `connect_remote` closes that gap instead of leaving a live credential
/// un-gitignored on a repo that's already been bootstrapped once.
pub fn ensure_gitignore_entries(repo: &Path) -> Result<()> {
    let gitignore_path = repo.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    let needed = [".context/"];
    let missing: Vec<&str> = needed.into_iter().filter(|e| !existing.lines().any(|l| l.trim() == *e)).collect();
    if missing.is_empty() {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("\n# agentops generated scan output\n");
    for entry in missing {
        updated.push_str(entry);
        updated.push('\n');
    }

    std::fs::write(&gitignore_path, updated)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_agents_md_with_resolved_notes_path_marker() {
        let dir = tempfile::tempdir().unwrap();
        let result = init_agents_md(dir.path(), None).unwrap();

        let content = std::fs::read_to_string(&result.agents_md_path).unwrap();
        assert!(content.contains("NOTES_PATH:"), "{content}");
        assert_eq!(result.notes_path, dir.path().join(".agentops").join("notes"));
    }

    #[test]
    fn explicit_notes_path_is_persisted_and_reflected_in_agents_md() {
        let dir = tempfile::tempdir().unwrap();
        let external = dir.path().join("external-vault");
        let result = init_agents_md(dir.path(), Some(&external)).unwrap();

        assert_eq!(result.notes_path, external);
        let content = std::fs::read_to_string(&result.agents_md_path).unwrap();
        assert!(content.contains(&format!("NOTES_PATH: {}", external.display())), "{content}");

        // Re-resolving without an override must pick up the persisted config.
        assert_eq!(agentops_notes::resolve_notes_path(dir.path(), None), external);
    }

    #[test]
    fn adds_context_to_gitignore_without_duplicating_on_rerun() {
        let dir = tempfile::tempdir().unwrap();
        init_agents_md(dir.path(), None).unwrap();
        init_agents_md(dir.path(), None).unwrap();

        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(gitignore.matches(".context/").count(), 1, "{gitignore}");
    }

    #[test]
    fn preserves_existing_gitignore_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "node_modules/\n").unwrap();
        init_agents_md(dir.path(), None).unwrap();

        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("node_modules/"));
        assert!(gitignore.contains(".context/"));
    }
}
