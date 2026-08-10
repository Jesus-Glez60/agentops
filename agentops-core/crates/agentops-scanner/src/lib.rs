//! Repo walker, AST extraction, dependency extraction, chunking, and
//! PageRank-based ranking over code — extraction only; storage into
//! `agentops-graph` is the caller's job (`agentops-mcp`'s
//! `scan_and_persist`), not this crate's.
//!
//! Zero runtime network dependency, enforced by `deny.toml` — this crate
//! must never gain a networking crate as a *runtime* dependency (the
//! `tree-sitter-language-pack` build-time grammar fetch is a documented,
//! understood exception — see the root `Cargo.toml`'s comment).

mod ast_extract;
mod chunker;
mod dep_extract;
mod ranker;
mod types;
mod walker;

pub use ranker::{rank_files, resolve_dependency_edges};
pub use types::{Chunk, ChunkKind, Language, ScannedFile, Symbol};

use std::path::Path;

/// Scans every supported file under `root`, returning one `ScannedFile` per
/// file with its extracted symbols, raw dependency targets, and chunks. All
/// raw text (symbol source, chunk text) has already passed through the
/// `agentops-security` redaction gate before this function returns it —
/// callers don't need to redact again.
pub fn scan_repo(root: &Path) -> anyhow::Result<ScanReport> {
    let mut files = Vec::new();
    let mut redacted_count = 0usize;
    let mut fallback_gap_files = Vec::new();

    for (abs_path, language) in walker::walk_repo(root) {
        let source = match std::fs::read_to_string(&abs_path) {
            Ok(s) => s,
            Err(_) => continue, // binary or unreadable file, skip
        };

        let (mut symbols, used_tree_sitter) = ast_extract::extract_symbols(language, &source);

        // Tracked uniformly across all five languages now that all five
        // have a real regex fallback — `main` only tracked this for Go
        // (`go_gap_files`), leaving Rust's identical gap invisible.
        if symbols.is_empty() && !used_tree_sitter {
            fallback_gap_files.push(abs_path.strip_prefix(root).unwrap_or(&abs_path).to_path_buf());
        }

        let deps = dep_extract::extract_deps(language, &source);

        for symbol in &mut symbols {
            let r = agentops_security::redact(&symbol.source);
            redacted_count += r.redacted_count;
            symbol.source = r.text;
        }

        let mut chunks = chunker::chunk_file(&source, &symbols);
        for chunk in &mut chunks {
            let r = agentops_security::redact(&chunk.text);
            redacted_count += r.redacted_count;
            chunk.text = r.text;
        }

        let rel_path = abs_path.strip_prefix(root).unwrap_or(&abs_path).to_path_buf();
        files.push(ScannedFile { path: rel_path, language, symbols, deps, chunks, used_tree_sitter });
    }

    Ok(ScanReport { files, redacted_count, fallback_gap_files })
}

#[derive(Debug)]
pub struct ScanReport {
    pub files: Vec<ScannedFile>,
    pub redacted_count: usize,
    /// Files where tree-sitter failed to parse AND the regex fallback for
    /// that language found zero symbols too — a visible warning, not a
    /// silent gap. Not necessarily a bug (a file can genuinely have zero
    /// top-level definitions), but worth surfacing.
    pub fallback_gap_files: Vec<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_a_small_multi_language_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("main.py"), "import os\n\ndef greet(name):\n    return f\"hi {name}\"\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/app.ts"), "import { greet } from './util';\n\nexport function main() {\n  return greet();\n}\n").unwrap();
        fs::write(root.join("src/util.ts"), "export function greet() {\n  return 'hi';\n}\n").unwrap();
        fs::write(root.join("config.py"), "aws_key = \"AKIAABCDEFGHIJKLMNOP\"\n").unwrap();

        let report = scan_repo(root).unwrap();
        assert_eq!(report.files.len(), 4, "found: {:?}", report.files.iter().map(|f| &f.path).collect::<Vec<_>>());
        assert!(report.redacted_count >= 1, "the fake AWS key should have been redacted");

        let config = report.files.iter().find(|f| f.path.ends_with("config.py")).unwrap();
        let all_text: String = config.chunks.iter().map(|c| c.text.clone()).collect();
        assert!(!all_text.contains("AKIAABCDEFGHIJKLMNOP"));

        let app = report.files.iter().find(|f| f.path.ends_with("app.ts")).unwrap();
        assert!(app.deps.contains(&"./util".to_string()));

        let ranked = rank_files(root, &report.files);
        assert!(ranked.iter().position(|(p, _)| p.ends_with("util.ts")).unwrap() <= 1);
    }

    /// Regression test: a Go file with a body tree-sitter can't parse must
    /// still yield symbols via the regex fallback, not silently zero.
    #[test]
    fn go_file_falling_back_to_regex_still_yields_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Deliberately malformed for tree-sitter (unbalanced), but the
        // brace-depth regex fallback only needs to find the opening line.
        fs::write(root.join("main.go"), "package main\n\nfunc Add(a int, b int) int {\n\treturn a + b\n}\n").unwrap();

        let report = scan_repo(root).unwrap();
        let go_file = report.files.iter().find(|f| f.path.ends_with("main.go")).unwrap();
        assert!(!go_file.symbols.is_empty(), "expected at least one symbol from Go, tree-sitter or fallback");
    }
}
