use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Python,
    TypeScript,
    JavaScript,
    Go,
    Rust,
}

impl Language {
    /// Detects a language from a file extension (no leading dot).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "py" => Some(Language::Python),
            "ts" | "tsx" => Some(Language::TypeScript),
            "js" | "jsx" | "mjs" => Some(Language::JavaScript),
            "go" => Some(Language::Go),
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    /// The identifier string `tree-sitter-language-pack` expects.
    pub fn tree_sitter_name(&self) -> &'static str {
        match self {
            Language::Python => "python",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Go => "go",
            Language::Rust => "rust",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    /// The enclosing scope's name, if any (e.g. `NodeKind` for a method
    /// found inside `impl NodeKind { ... }`) — disambiguates same-named
    /// symbols within one file (a confirmed real collision: multiple
    /// `impl` blocks in the same file commonly reuse method names like
    /// `new`/`fmt`/`as_db_str`). `name` itself stays bare so
    /// `agentops-notes`' word-boundary matching against freeform note text
    /// keeps matching on the identifier a human would actually type.
    /// Only populated via the tree-sitter path — the regex fallback
    /// doesn't track enclosing scope.
    pub container: Option<String>,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkKind {
    FileHeader,
    Symbol,
    Window,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub name: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannedFile {
    /// Path relative to the repo root.
    pub path: PathBuf,
    pub language: Language,
    pub symbols: Vec<Symbol>,
    /// Raw import/dependency targets as written in the source (not yet
    /// resolved to actual files — see `ranker::rank_files`).
    pub deps: Vec<String>,
    pub chunks: Vec<Chunk>,
    /// True if tree-sitter AST extraction succeeded; false means the regex
    /// fallback was used (which may itself have found zero symbols — see
    /// `ScanReport::fallback_gap_files`).
    pub used_tree_sitter: bool,
}
