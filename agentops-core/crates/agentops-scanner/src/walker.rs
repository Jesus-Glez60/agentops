use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::types::Language;

/// Directories never walked into — build artifacts, VCS internals,
/// dependency caches, and AgentOps's own output (`.context`/`.agentops` —
/// never source, but also never worth descending into for the same
/// "don't waste time/watch-handles on this" reasoning as everything else
/// here; see `watchable_dirs`, Phase 5's file watcher).
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

/// Walks `root` and returns every file with a supported language extension,
/// skipping excluded directories and secret-bearing filenames (see
/// `agentops_security::is_secret_bearing_filename`). Paths returned are
/// absolute; callers strip the repo root for storage.
pub fn walk_repo(root: &Path) -> Vec<(PathBuf, Language)> {
    let mut out = Vec::new();

    let walker = WalkDir::new(root).into_iter().filter_entry(|entry| {
        if entry.file_type().is_dir() {
            let name = entry.file_name().to_string_lossy();
            return !EXCLUDED_DIRS.contains(&name.as_ref());
        }
        true
    });

    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
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
/// `walk_repo` would actually descend into — i.e. `EXCLUDED_DIRS` already
/// pruned out, at any depth, not just the top level. Built for Phase 5's
/// file watcher: watching `root` with `notify`'s naive
/// `RecursiveMode::Recursive` also registers (and, on first start,
/// synchronously enumerates) every file under `target`/`node_modules`/etc,
/// which is both wasted work and, on a real repo with a populated build
/// dir, a genuine CPU/latency problem — confirmed live against this
/// project's own `target/` directories, not a hypothetical concern. The
/// caller watches each returned directory individually with
/// `RecursiveMode::NonRecursive` instead of one `Recursive` watch on
/// `root`, so an excluded subtree's contents are never touched by the
/// watch registration at all.
pub fn watchable_dirs(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| if entry.file_type().is_dir() { !EXCLUDED_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()) } else { true })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
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
}
