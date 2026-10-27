//! `connect --remote` in a repo other than AgentOps itself (milestone 5
//! item 2): a client's or another project's checkout, where nothing
//! tracked may change and `git status` must stay clean. Hooks go to the
//! untracked `.claude/settings.local.json`, the remote marker to
//! `.context/agentops-remote.json`, and either one not already ignored is
//! added to the repo's `.git/info/exclude`, which is local and never
//! committed. Nothing writes `AGENTS.md`/`CLAUDE.md`, `.gitignore` or a
//! Ruler pack; the MCP entry is user-level. A remote that matches no
//! connection is registered memory-only.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::remote_marker::read_remote_marker;

/// The paths a guest connect writes inside the repo, relative to its root.
pub(crate) const LOCAL_PATHS: &[&str] = &[".context/", ".claude/settings.local.json"];

/// Whether `path` is AgentOps' own checkout, which keeps the full connect
/// (tracked `settings.json` hooks, AGENTS.md, Ruler). Decided from the
/// origin remote alone, before any connection is resolved.
pub(crate) fn is_agentops_checkout(path: &Path) -> bool {
    crate::git_remote_url(path).as_deref().and_then(agentops_repo_access::normalize_repo_path).is_some_and(|p| p.eq_ignore_ascii_case("Jesus-Glez60/agentops"))
}

fn git(path: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git").arg("-C").arg(path).args(args).output().with_context(|| format!("running git {}", args.join(" ")))
}

/// Adds each of `paths` that the repo at `path` doesn't already ignore to
/// its `info/exclude`. Returns the paths added. Does nothing when `path`
/// isn't the top of a git checkout: a plain folder has no `git status` to
/// keep clean, and a folder inside a parent repo must not edit the
/// parent's exclude file. A path git already tracks is an error, since
/// writing it would show in `git status` whatever the exclude says.
pub(crate) fn exclude_untracked_local(path: &Path, paths: &[&str]) -> Result<Vec<String>> {
    let top = git(path, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Ok(Vec::new());
    }
    let top = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    if top.canonicalize().ok() != path.canonicalize().ok() {
        return Ok(Vec::new());
    }
    let mut added = Vec::new();
    for &p in paths {
        let tracked = git(path, &["ls-files", "--error-unmatch", p])?;
        anyhow::ensure!(!tracked.status.success(), "{p} is tracked in {}; guest connect would change a tracked file, so nothing was written", path.display());
        let ignored = git(path, &["check-ignore", "-q", p])?;
        match ignored.status.code() {
            Some(0) => continue,
            Some(1) => added.push(p.to_string()),
            _ => anyhow::bail!("git check-ignore {p} failed in {}: {}", path.display(), String::from_utf8_lossy(&ignored.stderr).trim()),
        }
    }
    if added.is_empty() {
        return Ok(added);
    }
    let exclude = git(path, &["rev-parse", "--git-path", "info/exclude"])?;
    anyhow::ensure!(exclude.status.success(), "git rev-parse --git-path info/exclude failed in {}", path.display());
    let exclude = path.join(String::from_utf8_lossy(&exclude.stdout).trim());
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    let mut text = existing.clone();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for p in &added {
        if !existing.lines().any(|l| l.trim() == p) {
            text.push_str(p);
            text.push('\n');
        }
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&exclude, text).with_context(|| format!("writing {}", exclude.display()))?;
    Ok(added)
}

/// Options a guest connect takes from `connect`'s flags.
pub(crate) struct GuestOptions<'a> {
    pub agents: &'a [String],
    pub with_hooks: bool,
    pub compress_commands: bool,
    pub no_work_recall: bool,
    /// Where the user-level MCP entry goes; the real home outside tests.
    pub home: &'a Path,
}

/// The files a guest connect wrote, for the person (and milestone 5's
/// per-repo report).
#[derive(Debug, Default)]
pub(crate) struct GuestWrites {
    pub excluded: Vec<String>,
    pub in_repo: Vec<PathBuf>,
    pub outside_repo: Vec<PathBuf>,
}

/// The guest connect, once the API key is known. `resolve` finds or
/// registers the connection (`resolve_connection_id` in guest mode); it is
/// skipped when the marker already names a connection that still exists.
pub(crate) fn connect_guest(path: &Path, server_url: &str, api_key: &str, opts: &GuestOptions, resolve: impl FnOnce() -> Result<String>, marker_is_live: impl FnOnce(&str) -> Result<bool>) -> Result<(String, GuestWrites)> {
    // The exclude comes first: the marker holds the API key, and nothing
    // may land in the repo before git is told to ignore it.
    let mut writes = GuestWrites { excluded: exclude_untracked_local(path, LOCAL_PATHS)?, ..Default::default() };
    let connection_id = match read_remote_marker(path) {
        Some(marker) if marker.server_url == server_url && marker_is_live(&marker.connection_id)? => marker.connection_id,
        _ => resolve()?,
    };
    crate::write_remote_marker(path, server_url, &connection_id, api_key)?;
    writes.in_repo.push(crate::remote_marker::remote_marker_path(path));
    if opts.with_hooks && opts.agents.iter().any(|a| a == "claude") {
        let settings = path.join(".claude").join("settings.local.json");
        crate::install_claude_hooks_at(&settings, opts.agents, opts.compress_commands, opts.no_work_recall)?;
        writes.in_repo.push(settings);
    }
    crate::write_user_level_mcp_entries_in(opts.home, opts.agents, None, Some((server_url, api_key)));
    writes.outside_repo.extend(opts.agents.iter().filter_map(|a| crate::user_level_mcp_config_path_in(opts.home, a)));
    Ok((connection_id, writes))
}

/// `agentops memory-off`: turns Claude Code's own auto-memory off, in the
/// repo's untracked `settings.local.json` or, with `user`, in
/// `~/.claude/settings.json`. Run after the MEMORY.md import, once its
/// records are checked, so nothing is lost while memory is still on.
pub(crate) fn memory_off(settings_path: &Path) -> Result<()> {
    crate::update_settings_json(settings_path, |root| {
        root.insert("autoMemoryEnabled".into(), serde_json::Value::Bool(false));
    })?;
    println!("Auto-memory is off in {}.", settings_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_git(dir: &Path, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A committed repo with the files a guest connect must never touch.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q"]);
        run_git(dir.path(), &["config", "user.email", "t@example.com"]);
        run_git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("AGENTS.md"), "# theirs\n").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# theirs\n").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    fn status(dir: &Path) -> String {
        String::from_utf8(Command::new("git").arg("-C").arg(dir).args(["status", "--porcelain", "--untracked-files=all"]).output().unwrap().stdout).unwrap()
    }

    fn opts<'a>(agents: &'a [String], home: &'a Path) -> GuestOptions<'a> {
        GuestOptions { agents, with_hooks: true, compress_commands: false, no_work_recall: false, home }
    }

    #[test]
    fn a_guest_connect_leaves_git_status_clean_and_writes_hooks_to_local_settings() {
        let dir = repo();
        let home = tempfile::tempdir().unwrap();
        let agents = vec!["claude".to_string()];
        let (id, writes) = connect_guest(dir.path(), "https://srv", "k", &opts(&agents, home.path()), || Ok("conn-1".into()), |_| Ok(true)).unwrap();
        assert_eq!(id, "conn-1");
        assert_eq!(status(dir.path()), "", "nothing tracked or untracked shows");
        assert_eq!(writes.excluded, vec![".context/".to_string(), ".claude/settings.local.json".to_string()]);
        assert_eq!(std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(), "# theirs\n");
        assert!(!dir.path().join(".claude").join("settings.json").exists());

        // The same hooks the tracked-file writer installs.
        let full = tempfile::tempdir().unwrap();
        crate::maybe_install_claude_hooks(full.path(), &agents, false, false).unwrap();
        let read = |p: PathBuf| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(read(dir.path().join(".claude").join("settings.local.json")), read(full.path().join(".claude").join("settings.json")));

        let mcp = read(home.path().join(".claude.json"));
        assert_eq!(mcp["mcpServers"]["agentops"]["url"].as_str().map(|u| u.starts_with("https://srv")), Some(true), "{mcp}");
        assert_eq!(writes.outside_repo, vec![home.path().join(".claude.json")]);

        // Again: nothing appended twice, and the marker's live connection is reused.
        let (again, writes) = connect_guest(dir.path(), "https://srv", "k", &opts(&agents, home.path()), || panic!("must reuse the marker"), |id| Ok(id == "conn-1")).unwrap();
        assert_eq!((again.as_str(), writes.excluded.len()), ("conn-1", 0));
        let exclude = std::fs::read_to_string(dir.path().join(".git").join("info").join("exclude")).unwrap();
        assert_eq!(exclude.lines().filter(|l| *l == ".context/").count(), 1, "{exclude}");
        assert_eq!(status(dir.path()), "");
    }

    #[test]
    fn a_marker_on_a_connection_that_no_longer_exists_is_resolved_again() {
        let dir = repo();
        let home = tempfile::tempdir().unwrap();
        crate::write_remote_marker(dir.path(), "https://srv", "dead-stub", "k").unwrap();
        let (id, _) = connect_guest(dir.path(), "https://srv", "k", &opts(&[], home.path()), || Ok("live".into()), |_| Ok(false)).unwrap();
        assert_eq!(id, "live");
        assert_eq!(read_remote_marker(dir.path()).unwrap().connection_id, "live");
    }

    #[test]
    fn an_already_ignored_path_is_not_excluded_and_a_tracked_one_aborts_before_any_write() {
        let dir = repo();
        std::fs::write(dir.path().join(".gitignore"), "target/\n.context/\n").unwrap();
        run_git(dir.path(), &["commit", "-qam", "ignore context"]);
        assert_eq!(exclude_untracked_local(dir.path(), LOCAL_PATHS).unwrap(), vec![".claude/settings.local.json".to_string()]);

        let tracked = repo();
        std::fs::create_dir_all(tracked.path().join(".claude")).unwrap();
        std::fs::write(tracked.path().join(".claude").join("settings.local.json"), "{}").unwrap();
        run_git(tracked.path(), &["add", "-f", "."]);
        run_git(tracked.path(), &["commit", "-qm", "tracked local settings"]);
        let home = tempfile::tempdir().unwrap();
        let err = connect_guest(tracked.path(), "https://srv", "k", &opts(&["claude".to_string()], home.path()), || Ok("c".into()), |_| Ok(true)).unwrap_err();
        assert!(err.to_string().contains("is tracked"), "{err}");
        assert!(!tracked.path().join(".context").exists(), "no marker (API key) written");
        assert_eq!(status(tracked.path()), "");
    }

    #[test]
    fn a_plain_folder_or_one_inside_a_parent_repo_gets_no_exclude() {
        let plain = tempfile::tempdir().unwrap();
        assert!(exclude_untracked_local(plain.path(), LOCAL_PATHS).unwrap().is_empty());

        let parent = repo();
        let nested = parent.path().join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(exclude_untracked_local(&nested, LOCAL_PATHS).unwrap().is_empty());
        let exclude = std::fs::read_to_string(parent.path().join(".git").join("info").join("exclude")).unwrap_or_default();
        assert!(!exclude.contains(".context/"), "the parent's exclude is untouched: {exclude}");
    }

    #[test]
    fn local_settings_keep_existing_keys_and_a_malformed_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join(".claude").join("settings.local.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"theirs"}]}]}}"#).unwrap();
        crate::install_claude_hooks_at(&settings, &["claude".to_string()], false, false).unwrap();
        memory_off(&settings).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "theirs");
        assert!(v["hooks"]["UserPromptSubmit"].is_array());
        assert_eq!(v["autoMemoryEnabled"], false);

        std::fs::write(&settings, "{ not json").unwrap();
        assert!(memory_off(&settings).is_err());
        assert!(crate::install_claude_hooks_at(&settings, &["claude".to_string()], false, false).is_err());
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{ not json", "never replaced with {{}}");
    }
}
