use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::types::Language;

/// Directories never walked into — build artifacts, VCS internals, dependency
/// caches. Mirrors codebrain's `index_repo.py` exclude list, plus `target/` for
/// Rust since this scanner itself is Rust.
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
];

/// Walks `root` and returns every file with a supported language extension,
/// skipping excluded directories and secret-bearing filenames (`.env`, `*.pem`,
/// `*.key`, `id_rsa*`, ... — see `agentops_security::is_secret_bearing_filename`).
/// Paths returned are absolute; callers strip the repo root for storage.
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
        fs::write(root.join("README.md"), "# hi").unwrap(); // unsupported ext, skipped
        fs::write(root.join(".env"), "SECRET=1").unwrap(); // secret-bearing, skipped

        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules/lib.js"), "module.exports = {};").unwrap();

        let found = walk_repo(root);
        let names: Vec<String> = found
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(names.contains(&"main.py".to_string()));
        assert!(names.contains(&"app.ts".to_string()));
        assert!(!names.contains(&"README.md".to_string()));
        assert!(!names.contains(&".env".to_string()));
        assert!(!names.contains(&"lib.js".to_string()), "node_modules must be excluded");
    }
}
