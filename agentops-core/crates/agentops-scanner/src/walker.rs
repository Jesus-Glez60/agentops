use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::types::Language;

/// Directories never walked into, layered on top of whatever a repo's own
/// `.gitignore` already excludes (see `walk_repo`/`watchable_dirs` below) —
/// build artifacts, VCS internals, dependency caches, and AgentOps's own
/// output (`.context`/`.agentops` — never source, but also never worth
/// descending into for the same "don't waste time/watch-handles on this"
/// reasoning as everything else here; see `watchable_dirs`, Phase 5's file
/// watcher). Kept as an always-applied baseline so a repo with no
/// `.gitignore`, or one that doesn't bother ignoring its own build output,
/// still gets these sane defaults.
const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    "dist",
    "build",
    "venv",
    ".venv",
    "vendor",
    ".next",
    "out",
    "coverage",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
    "target",
    ".context",
    ".agentops",
];

fn excludes_baseline_dirs(entry: &ignore::DirEntry) -> bool {
    if entry.file_type().is_some_and(|ft| ft.is_dir()) {
        let name = entry.file_name().to_string_lossy();
        return !EXCLUDED_DIRS.contains(&name.as_ref());
    }
    true
}

/// Builds an `ignore::Walk` rooted at `root`: respects `.gitignore`,
/// `.git/info/exclude`, and global git excludes (via the `ignore` crate,
/// ripgrep's — verified clean against `deny.toml`'s zero-runtime-network-
/// egress ban via `cargo deny check bans` before adopting it), with
/// `EXCLUDED_DIRS` layered on top as an always-applied baseline. Hidden
/// files/dirs are explicitly *not* skipped (`ignore`'s own default) --
/// today's `EXCLUDED_DIRS`-only filtering never excluded dotfiles beyond
/// `.git`/`.context`/`.agentops`, so leaving `ignore`'s default in place
/// would silently start hiding things like a tracked `.github/` that were
/// always scanned before.
fn build_walker(root: &Path) -> ignore::Walk {
    WalkBuilder::new(root).hidden(false).filter_entry(excludes_baseline_dirs).build()
}

/// Walks `root` and returns every file with a supported language extension,
/// skipping excluded directories, anything `.gitignore`-excluded, and
/// secret-bearing filenames (see
/// `agentops_security::is_secret_bearing_filename`). Paths returned are
/// absolute; callers strip the repo root for storage.
pub fn walk_repo(root: &Path) -> Vec<(PathBuf, Language)> {
    let mut out = Vec::new();

    for entry in build_walker(root).filter_map(|e| e.ok()) {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();

        if agentops_security::is_secret_bearing_filename(path) {
            continue;
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if let Some(lang) = Language::from_extension(ext) {
            out.push((path.to_path_buf(), lang));
        }
    }

    out
}

/// Every directory under `root` (including `root` itself) that
/// `walk_repo` would actually descend into — i.e. `EXCLUDED_DIRS` and
/// `.gitignore` already pruned out, at any depth, not just the top level.
/// Built for Phase 5's file watcher: watching `root` with `notify`'s naive
/// `RecursiveMode::Recursive` also registers (and, on first start,
/// synchronously enumerates) every file under `target`/`node_modules`/etc,
/// which is both wasted work and, on a real repo with a populated build
/// dir, a genuine CPU/latency problem — confirmed live against this
/// project's own `target/` directories, not a hypothetical concern. The
/// caller watches each returned directory individually with
/// `RecursiveMode::NonRecursive` instead of one `Recursive` watch on
/// `root`, so an excluded subtree's contents are never touched by the
/// watch registration at all. Kept on the same `.gitignore`-aware walker
/// as `walk_repo` so the two never diverge on what counts as "in scope".
pub fn watchable_dirs(root: &Path) -> Vec<PathBuf> {
    build_walker(root)
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_dir()))
        .map(|e| e.path().to_path_buf())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_supported_files_and_skips_excluded_dirs_and_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("main.py"), "def f(): pass").unwrap();
        fs::write(root.join("app.ts"), "export const x = 1;").unwrap();
        fs::write(root.join("README.md"), "# hi").unwrap();
        fs::write(root.join(".env"), "SECRET=1").unwrap();

        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/lib.js"), "module.exports = {};").unwrap();

        let found = walk_repo(root);
        let names: Vec<String> = found.iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();

        assert!(names.contains(&"main.py".to_string()));
        assert!(names.contains(&"app.ts".to_string()));
        assert!(!names.contains(&"README.md".to_string()));
        assert!(!names.contains(&".env".to_string()));
        assert!(!names.contains(&"lib.js".to_string()), "node_modules must be excluded");
    }

    #[test]
    fn watchable_dirs_excludes_build_artifact_trees_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target/debug/deps")).unwrap();
        fs::create_dir_all(root.join("nested/node_modules/pkg")).unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join(".context")).unwrap();

        let dirs = watchable_dirs(root);
        let names: Vec<String> = dirs.iter().map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned()).collect();

        assert!(names.contains(&"src".to_string()), "found: {names:?}");
        assert!(names.contains(&"nested".to_string()), "a non-excluded parent must still be watched: {names:?}");
        assert!(!names.iter().any(|n| n.contains("target")), "must never descend into target: {names:?}");
        assert!(!names.iter().any(|n| n.contains("node_modules")), "must never descend into a nested node_modules: {names:?}");
        assert!(!names.iter().any(|n| n.contains(".git")), "must never descend into .git: {names:?}");
        assert!(!names.iter().any(|n| n.contains(".context")), "must never descend into .context: {names:?}");
    }

    #[test]
    fn respects_a_repo_gitignore_for_a_directory_not_in_excluded_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // The `ignore` crate only honors `.gitignore` inside an actual git
        // repo by default (`require_git`) -- every repo this product
        // actually scans is a real git checkout (GitHub App/SSH clone via
        // `agentops-repo-access`), so this matches production, not a
        // relaxed test-only assumption.
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "fixtures/\n").unwrap();
        fs::create_dir_all(root.join("fixtures")).unwrap();
        fs::write(root.join("fixtures/data.py"), "x = 1").unwrap();
        fs::write(root.join("main.py"), "def f(): pass").unwrap();

        let found = walk_repo(root);
        let names: Vec<String> = found.iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();

        assert!(names.contains(&"main.py".to_string()));
        assert!(!names.contains(&"data.py".to_string()), "a .gitignore'd dir not in EXCLUDED_DIRS must still be skipped: {names:?}");
    }

    #[test]
    fn still_scans_dotfiles_and_dotdirs_not_git_ignored() {
        // `ignore`'s own default hides dotfiles/dirs; today's behavior never
        // did (only .git/.context/.agentops are excluded), so this must
        // keep working exactly as before the switch to the `ignore` crate.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join(".github/workflows/ci.py"), "x = 1").unwrap();

        let found = walk_repo(root);
        let names: Vec<String> = found.iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"ci.py".to_string()), "a tracked dotdir not in EXCLUDED_DIRS must still be scanned: {names:?}");
    }
}
