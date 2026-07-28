//! Generates a `.ruler/` source directory from the embedded prompt pack and
//! shells out to a pinned version of `@intellectronica/ruler` to fan it out to
//! whichever agents (Claude Code, Cursor, etc.) are selected.
//!
//! Verified empirically against Ruler 0.3.44 (not assumed from its README):
//! `.ruler/agents/<name>.md` files become distinct Claude Code subagents
//! (`ruler apply --subagents`), copied into `.claude/agents/` largely as-is —
//! but Ruler does NOT resolve `@include` directives (that's a Claude-Code-only
//! convention dev-agent-ops's own install script never resolved either, just
//! copied verbatim); this bridge resolves them itself before handing anything
//! to Ruler, so the files Ruler distributes are fully self-contained.
//! `.ruler/skills/<name>/SKILL.md` (directory-per-skill, not a flat `.md`) is
//! the format Ruler actually recognizes — a flat `.ruler/skills/<name>.md`
//! produced "No valid skills found" in the same empirical check.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};

/// The prompt pack, embedded into the binary at compile time — no separate
/// asset directory needs to ship alongside the release binary.
static PROMPTS: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../prompts");

/// Pinned Ruler version — verified against this exact version's CLI surface.
/// Bump deliberately, not via an unpinned `npx @intellectronica/ruler@latest`
/// (see SECURITY.md's supply-chain rationale).
pub const RULER_VERSION: &str = "0.3.44";

/// Resolves `@include <relative-path>` lines against the embedded prompt pack,
/// inlining the referenced file's content in place of the directive.
fn resolve_includes(source: &str, from_dir: &str) -> Result<String> {
    let mut out = String::new();
    for line in source.lines() {
        if let Some(rel) = line.trim().strip_prefix("@include ") {
            let resolved_path = normalize_join(from_dir, rel.trim());
            let file = PROMPTS
                .get_file(&resolved_path)
                .with_context(|| format!("@include target not found in prompt pack: {resolved_path}"))?;
            out.push_str(file.contents_utf8().unwrap_or_default());
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Joins `base_dir` with a `../`-style relative path and lexically normalizes
/// the result, without touching the filesystem (the embedded `Dir` isn't one).
fn normalize_join(base_dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for component in rel.split('/') {
        match component {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Builds `<target_repo>/.ruler/` from the embedded prompt pack:
/// - `.ruler/AGENTS.md` = `agents_md_content` (the same content already written
///   to the repo root by `agentops-agents-md`, reused as Ruler's rules source)
/// - `.ruler/ruler.toml` (minimal, defaults are fine for Phase 1)
/// - `.ruler/agents/<name>.md` — every `prompts/agents/*.md`, `@include`-resolved
/// - `.ruler/skills/<name>/SKILL.md` — every `prompts/skills/<name>.md`
pub fn build_ruler_dir(target_repo: &Path, agents_md_content: &str) -> Result<()> {
    let ruler_dir = target_repo.join(".ruler");
    std::fs::create_dir_all(&ruler_dir)?;

    std::fs::write(ruler_dir.join("AGENTS.md"), agents_md_content)?;
    std::fs::write(
        ruler_dir.join("ruler.toml"),
        "# Managed by agentops — see agentops-ruler-bridge. Safe to hand-edit;\n\
         # re-running `agentops install` will not overwrite agent-specific overrides\n\
         # you add below this line.\n",
    )?;

    let agents_dir = ruler_dir.join("agents");
    std::fs::create_dir_all(&agents_dir)?;
    if let Some(dir) = PROMPTS.get_dir("agents") {
        for file in dir.files() {
            let name = file.path().file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let source = file.contents_utf8().unwrap_or_default();
            let resolved = resolve_includes(source, "agents")?;
            std::fs::write(agents_dir.join(name), resolved)?;
        }
    }

    let skills_dir = ruler_dir.join("skills");
    std::fs::create_dir_all(&skills_dir)?;
    if let Some(dir) = PROMPTS.get_dir("skills") {
        for file in dir.files() {
            let stem = file.path().file_stem().and_then(|n| n.to_str()).unwrap_or_default();
            let skill_subdir = skills_dir.join(stem);
            std::fs::create_dir_all(&skill_subdir)?;
            std::fs::write(skill_subdir.join("SKILL.md"), file.contents_utf8().unwrap_or_default())?;
        }
    }

    Ok(())
}

/// Confirms `npx` is available before attempting anything — Node is a
/// documented peer-dependency solely for this step (see the plan's
/// "residual tension" note on Ruler being npm-only).
pub fn preflight_check_npx() -> Result<()> {
    let status = Command::new("npx").arg("--version").output();
    match status {
        Ok(out) if out.status.success() => Ok(()),
        _ => bail!(
            "npx not found (or failed to run) — Ruler-based prompt distribution requires Node.js. \
             Install Node (e.g. via nvm/fnm) and re-run, or skip this step with --no-ruler."
        ),
    }
}

/// Invokes the pinned Ruler version's `apply` command against `target_repo`'s
/// `.ruler/` directory. Returns combined stdout+stderr for the caller to print.
pub fn apply(target_repo: &Path, agent_ids: &[&str], dry_run: bool) -> Result<String> {
    preflight_check_npx()?;

    let mut args: Vec<String> = vec![
        "--yes".into(),
        format!("@intellectronica/ruler@{RULER_VERSION}"),
        "apply".into(),
        "--project-root".into(),
        target_repo.display().to_string(),
        // Subagents default to disabled in Ruler (experimental) — verified empirically
        // that without this flag, `.ruler/agents/*.md` never reach `.claude/agents/`
        // even though they're correctly generated. Skills are enabled by default.
        "--subagents".into(),
    ];
    if !agent_ids.is_empty() {
        args.push("--agents".into());
        args.push(agent_ids.join(","));
    }
    if dry_run {
        args.push("--dry-run".into());
    }

    let output = Command::new("npx")
        .args(&args)
        .output()
        .context("running npx @intellectronica/ruler apply")?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if !output.status.success() {
        bail!("ruler apply failed:\n{combined}");
    }

    Ok(combined)
}

/// The paths this bridge writes into a target repo — used by the CLI to seed
/// `.gitignore` if desired, mirroring the pattern already used for `.context/`.
pub fn ruler_dir_path(target_repo: &Path) -> PathBuf {
    target_repo.join(".ruler")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_include_directives_against_embedded_prompts() {
        let backend_source = PROMPTS
            .get_file("agents/backend.md")
            .expect("backend.md must be embedded")
            .contents_utf8()
            .unwrap();

        let resolved = resolve_includes(backend_source, "agents").unwrap();

        assert!(!resolved.contains("@include"), "no @include directive should remain");
        // constraints.md's actual heading should now be inlined.
        assert!(resolved.contains("GLOBAL CONSTRAINTS"), "constraints.md content should be inlined");
        assert!(resolved.contains("WORKFLOW CONTRACT"), "workflow-contract.md content should be inlined");
    }

    #[test]
    fn builds_ruler_dir_with_resolved_agents_and_skill_directories() {
        let dir = tempfile::tempdir().unwrap();
        build_ruler_dir(dir.path(), "# Test AGENTS.md\n").unwrap();

        assert!(dir.path().join(".ruler/AGENTS.md").exists());
        assert!(dir.path().join(".ruler/ruler.toml").exists());

        let backend = std::fs::read_to_string(dir.path().join(".ruler/agents/backend.md")).unwrap();
        assert!(!backend.contains("@include"));
        assert!(backend.contains("GLOBAL CONSTRAINTS"));

        // Skills must land as <name>/SKILL.md, not a flat <name>.md — verified
        // empirically against Ruler, which reports "No valid skills found"
        // for the flat form.
        assert!(dir.path().join(".ruler/skills/plan/SKILL.md").exists());
    }

    #[test]
    fn normalize_join_handles_parent_dir_references() {
        assert_eq!(normalize_join("agents", "../docs/constraints.md"), "docs/constraints.md");
        assert_eq!(normalize_join("agents", "../docs/domains/backend-analysis.md"), "docs/domains/backend-analysis.md");
    }
}
