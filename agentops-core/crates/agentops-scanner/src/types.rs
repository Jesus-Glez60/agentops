use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Python,
    TypeScript,
    JavaScript,
    Go,
}

impl Language {
    /// Detects a language from a file extension (no leading dot).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "py" => Some(Language::Python),
            "ts" | "tsx" => Some(Language::TypeScript),
            "js" | "jsx" | "mjs" => Some(Language::JavaScript),
            "go" => Some(Language::Go),
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
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
    /// Raw import/dependency targets as written in the source (not yet resolved
    /// to actual files — see `ranker::rank_files`).
    pub deps: Vec<String>,
    pub chunks: Vec<Chunk>,
    /// True if tree-sitter AST extraction succeeded; false means the regex
    /// fallback was used (or, for Go, that zero symbols were extracted — see
    /// SECURITY.md / the plan's note on the Go AST-fallback gap).
    pub used_tree_sitter: bool,
}
