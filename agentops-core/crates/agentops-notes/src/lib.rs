//! Use-case layer: markdown notes ingestion (a real vault or an unorganized
//! folder — see `vault.rs`), the `SymbolMatcher` port + `WordBoundaryMatcher`
//! adapter, and the `NoteClassifier` port + `HeuristicClassifier` adapter.

mod config;
mod vault;

pub use config::{default_notes_path, resolve_notes_path, write_notes_path};
pub use vault::{
    connect_many, content_with_tags, with_frontmatter_type, heuristic_score, ingest_vault, match_same_file_references, match_symbols, tags_of, walk_vault, HeuristicClassifier, TAGS_MARKER, IngestSummary, NoteClassifier,
    NoteType, SymbolMatcher, VaultNote, WordBoundaryMatcher,
};
