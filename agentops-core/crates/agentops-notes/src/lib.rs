//! Use-case layer: markdown notes ingestion (a real vault or an unorganized
//! folder — see `vault.rs`), the `SymbolMatcher` port + `WordBoundaryMatcher`
//! adapter, and the `NoteClassifier` port + `HeuristicClassifier` adapter.

mod config;
mod vault;

pub use config::{default_notes_path, resolve_notes_path, write_notes_path};
pub use vault::{
    connect_many, heuristic_score, ingest_vault, match_symbols, walk_vault, HeuristicClassifier, IngestSummary, NoteClassifier, NoteType, SymbolMatcher,
    VaultNote, WordBoundaryMatcher,
};
