//! Repo walker, AST extraction, dependency extraction, chunking, and PageRank-based
//! ranking over code — the "produce chunks/symbols/deps" half of the neuron model.
//! Storage (into `agentops-graph`) is the caller's job, not this crate's — see the
//! plan's §"Drop for light tier entirely" on decoupling extraction from storage.
//!
//! Zero runtime network dependency, enforced by `deny.toml` (see SECURITY.md) —
//! this crate must never gain a networking crate as a *runtime* dependency.

mod ast_extract;
mod chunker;
mod dep_extract;
mod ranker;
mod types;
mod walker;

pub use ranker::rank_files;
pub use types::{Chunk, ChunkKind, Language, ScannedFile, Symbol};

use std::path::Path;

/// Scans every supported file under `root`, returning one `ScannedFile` per file
/// with its extracted symbols, raw dependency targets, and chunks. All raw text
/// (symbol source, chunk text) has already passed through the
/// `agentops-security` redaction gate before this function returns it — callers
/// (e.g. `agentops-cli`'s `install` command writing into `agentops-graph`) don't
/// need to redact again.
///
/// Returns the scanned files plus a summary: total secrets redacted, and how
/// many files hit the Go AST-fallback gap (tree-sitter failed and there's no
/// regex fallback for Go — see SECURITY.md).
pub fn scan_repo(root: &Path) -> anyhow::Result<ScanReport> {
    let mut files = Vec::new();
    let mut redacted_count = 0usize;
    let mut go_gap_files = Vec::new();

    for (abs_path, language) in walker::walk_repo(root) {
        let source = match std::fs::read_to_string(&abs_path) {
            Ok(s) => s,
            Err(_) => continue, // binary or unreadable file, skip
        };

        let (mut symbols, used_tree_sitter) = ast_extract::extract_symbols(language, &source);
        let deps = dep_extract::extract_deps(language, &source);

        if language == Language::Go && symbols.is_empty() && !used_tree_sitter {
            go_gap_files.push(abs_path.strip_prefix(root).unwrap_or(&abs_path).to_path_buf());
        }

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
        files.push(ScannedFile {
            path: rel_path,
            language,
            symbols,
            deps,
            chunks,
            used_tree_sitter,
        });
    }

    Ok(ScanReport { files, redacted_count, go_gap_files })
}

#[derive(Debug)]
pub struct ScanReport {
    pub files: Vec<ScannedFile>,
    pub redacted_count: usize,
    /// Files where Go AST extraction failed and there's no fallback — a visible
    /// warning, not a silent gap (see SECURITY.md).
    pub go_gap_files: Vec<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_a_small_multi_language_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(
            root.join("main.py"),
            "import os\n\ndef greet(name):\n    return f\"hi {name}\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/app.ts"),
            "import { greet } from './util';\n\nexport function main() {\n  return greet();\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/util.ts"),
            "export function greet() {\n  return 'hi';\n}\n",
        )
        .unwrap();
        // A file with an embedded fake secret — must be redacted, not leaked.
        fs::write(
            root.join("config.py"),
            "aws_key = \"AKIAABCDEFGHIJKLMNOP\"\n",
        )
        .unwrap();

        let report = scan_repo(root).unwrap();
        assert_eq!(report.files.len(), 4, "found: {:?}", report.files.iter().map(|f| &f.path).collect::<Vec<_>>());
        assert!(report.redacted_count >= 1, "the fake AWS key should have been redacted");

        let config = report.files.iter().find(|f| f.path.ends_with("config.py")).unwrap();
        let all_text: String = config.chunks.iter().map(|c| c.text.clone()).collect();
        assert!(!all_text.contains("AKIAABCDEFGHIJKLMNOP"));

        let app = report.files.iter().find(|f| f.path.ends_with("app.ts")).unwrap();
        assert!(app.deps.contains(&"./util".to_string()));

        let ranked = rank_files(&report.files);
        // util.ts is imported by app.ts, so it should rank at or near the top.
        assert!(ranked.iter().position(|(p, _)| p.ends_with("util.ts")).unwrap() <= 1);
    }
}
