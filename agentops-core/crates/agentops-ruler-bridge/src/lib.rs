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

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir};

/// The prompt pack, embedded into the binary at compile time — no separate
/// asset directory needs to ship alongside the release binary. Layout:
/// `agents/*.md` (subagents), `docs/*.md` (protocol docs the subagents defer
/// to), `skills/*.md` (fanned to every tool's native skills dir via Ruler),
/// and `shared/*.md` (canonical instruction bodies, pulled into `skills/`
/// via `@include`).
///
/// Deliberately no separate `commands/*.md` / `.claude/commands/` layer:
/// Claude Code unified custom commands into Skills (a `.claude/skills/`
/// entry now shows up in the `/` menu *and* can be auto-invoked — a plain
/// legacy command file is strictly a subset), so writing both under the
/// same name produced two menu entries for one capability — caught live,
/// not a hypothetical. Skills alone now cover both Claude Code's `/` menu
/// and Cursor's auto-invocation (Cursor has no slash-command support at
/// all — an open feature request, not shipped).
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
///   to the repo root by `agentops-mcp::init_agents_md`, reused as Ruler's
///   rules source)
/// - `.ruler/ruler.toml` (minimal, defaults are fine for this pass)
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

    // `.ruler/skills/` must exist even when the prompt pack has no `skills/`
    // dir at all (as today) — `PROMPTS.get_dir("skills")` degrades
    // gracefully by simply writing no skill subdirectories.
    let skills_dir = ruler_dir.join("skills");
    std::fs::create_dir_all(&skills_dir)?;
    if let Some(dir) = PROMPTS.get_dir("skills") {
        for file in dir.files() {
            let stem = file.path().file_stem().and_then(|n| n.to_str()).unwrap_or_default();
            let source = file.contents_utf8().unwrap_or_default();
            let resolved = resolve_includes(source, "skills")?;
            let skill_subdir = skills_dir.join(stem);
            std::fs::create_dir_all(&skill_subdir)?;
            std::fs::write(skill_subdir.join("SKILL.md"), resolved)?;
        }
    }

    Ok(())
}

/// Confirms `npx` is available before attempting anything — Node is a
/// documented peer-dependency solely for this step.
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

/// Confirms `agentops-mcp-server` is on `PATH` before writing `.ruler/mcp.json`
/// that references it — otherwise every agent Ruler fans that config out to
/// (Claude Code's `.mcp.json`, Cursor's `.cursor/mcp.json`, etc.) ends up
/// pointing at a command that doesn't exist, failing silently until someone
/// actually tries to use an agentops tool from inside their coding agent.
/// Ships alongside `agentops`/`agentops-server` via `install.sh`/cargo-dist
/// on a classic install, or `cargo build --release --bin agentops-mcp-server`
/// from a source checkout — this just confirms one of those actually happened.
pub fn preflight_check_mcp_server_binary() -> Result<()> {
    let status = Command::new("agentops-mcp-server").arg("--help").output();
    match status {
        Ok(_) => Ok(()),
        Err(_) => bail!(
            "agentops-mcp-server not found on PATH — it ships alongside the `agentops` binary \
             (via install.sh, or `cargo build --release --bin agentops-mcp-server` from a source checkout). \
             Install it and re-run, or skip MCP registration and just distribute instructions with --no-ruler."
        ),
    }
}

/// Invokes the pinned Ruler version's `apply` command against `target_repo`'s
/// `.ruler/` directory. Returns combined stdout+stderr for the caller to print.
///
/// `with_mcp` is always `false` from `agentops-cli` now: MCP registration
/// moved to a user-level (`~/.claude.json`, `~/.cursor/mcp.json`, etc.)
/// direct write instead of a project-level `.ruler/mcp.json` Ruler fans out
/// — caught live: Cursor currently has an active regression where
/// project-level `.cursor/mcp.json` isn't read by its Customize UI at all
/// (confirmed via multiple reports on Cursor's own forum, not just this
/// install). Since `agentops-mcp-server`'s identity doesn't vary per repo
/// the way `AGENTS.md`/notes content does, one global registration per
/// machine is also just simpler, not merely a workaround.
pub fn apply(target_repo: &Path, agent_ids: &[&str], dry_run: bool, with_mcp: bool) -> Result<String> {
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
    if !with_mcp {
        args.push("--no-mcp".into());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_ruler_dir_with_the_actual_current_prompt_pack() {
        let dir = tempfile::tempdir().unwrap();
        build_ruler_dir(dir.path(), "# Test AGENTS.md\n").unwrap();

        assert!(dir.path().join(".ruler/AGENTS.md").exists());
        assert!(dir.path().join(".ruler/ruler.toml").exists());

        // Today's pack has no `@include` anywhere, so nothing to resolve —
        // the file lands byte-for-byte, still exercising the same code path.
        let archivist = std::fs::read_to_string(dir.path().join(".ruler/agents/vault-archivist.md")).unwrap();
        assert!(!archivist.is_empty());
        assert!(!archivist.contains("@include"));

        // Every `prompts/agents/*.md` file is picked up automatically, no
        // code change needed -- confirms ponytail-auditor.md (added
        // alongside vault-archivist.md) flows through the same path.
        assert!(dir.path().join(".ruler/agents/ponytail-auditor.md").exists());

        // Each `prompts/skills/<name>.md` becomes `.ruler/skills/<name>/SKILL.md`,
        // with its `@include ../shared/<name>.md` resolved -- this is the
        // ONLY artifact for these capabilities (see this crate's own doc
        // comment on why there's deliberately no separate `.claude/commands/`
        // layer: Claude Code unified commands into skills, so a plain
        // command file next to an identically-named skill just double-lists
        // the same capability in the `/` menu -- caught live, fixed by
        // removing the redundant layer rather than renaming around it).
        let ponytail_skill = std::fs::read_to_string(dir.path().join(".ruler/skills/ponytail-audit/SKILL.md")).unwrap();
        assert!(!ponytail_skill.contains("@include"), "skills must resolve @include, same as agents do");
        assert!(ponytail_skill.contains("reuse-before-writing decision ladder"), "the shared body's actual content must be inlined: {ponytail_skill:?}");
    }

    #[test]
    fn normalize_join_handles_parent_dir_references() {
        assert_eq!(normalize_join("agents", "../docs/vault-protocol.md"), "docs/vault-protocol.md");
        assert_eq!(normalize_join("agents", "../docs/domains/backend-analysis.md"), "docs/domains/backend-analysis.md");
    }
}
