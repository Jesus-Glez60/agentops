//! Shared "write/ingest a note" use cases — used by both the MCP `add_note`/
//! `ingest_notes` tool handlers (`tools.rs`) and `agentops-cli`'s `note`/
//! `ingest-notes` commands. Mirrors `scan.rs`'s role for `scan_and_persist`:
//! one implementation, multiple thin driving adapters, so the CLI and the
//! MCP tool can never drift apart on what "adding a note" actually does.

use std::path::{Path, PathBuf};

use agentops_graph::SqliteGraphStore;
use anyhow::Result;

use crate::scan::{graph_db_path, repo_name};

pub struct AddNoteResult {
    pub file_path: PathBuf,
    pub note_type: agentops_notes::NoteType,
    pub edges_written: usize,
}

/// Writes a new note to the repo's resolved notes folder and ingests it
/// into the graph in one call. `note_type` of `None` means "classify it" —
/// callers that already know the type (an explicit CLI flag, or a
/// caller-supplied `note_type` string) pass `Some(..)`, including
/// `Some(NoteType::Knowledge)` to skip classification for an explicit
/// "knowledge" request rather than treating it as ambiguous.
pub fn add_note(repo_path: &Path, title: &str, body: &str, note_type: Option<agentops_notes::NoteType>, tags: &[String], notes_path_override: Option<&Path>) -> Result<AddNoteResult> {
    let store = SqliteGraphStore::open(&graph_db_path(repo_path))?;
    let repo = repo_name(repo_path);
    let classifier = agentops_notes::HeuristicClassifier;
    let matcher = agentops_notes::WordBoundaryMatcher::default();

    let note_type = match note_type {
        Some(t) => t,
        None => {
            use agentops_notes::NoteClassifier;
            classifier.classify(body)?
        }
    };

    let notes_dir = agentops_notes::resolve_notes_path(repo_path, notes_path_override);
    std::fs::create_dir_all(&notes_dir)?;
    let slug = title.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>();
    let slug = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    let file_path = notes_dir.join(format!("{slug}.md"));

    let type_str = match note_type {
        agentops_notes::NoteType::Gotcha => "gotcha",
        agentops_notes::NoteType::Decision => "decision",
        agentops_notes::NoteType::Knowledge => "knowledge",
        agentops_notes::NoteType::Context => "context",
    };
    let tags_yaml = if tags.is_empty() { String::new() } else { format!("tags: [{}]\n", tags.join(", ")) };
    let content = format!("---\ntitle: \"{title}\"\ntype: {type_str}\n{tags_yaml}---\n\n{body}\n");
    std::fs::write(&file_path, &content)?;

    let notes = agentops_notes::walk_vault(&notes_dir, &classifier)?;
    let this_note = notes.into_iter().find(|n| n.title == title).ok_or_else(|| anyhow::anyhow!("wrote note but failed to re-parse it"))?;
    let summary = agentops_notes::ingest_vault(&store, &repo, std::slice::from_ref(&this_note), &matcher)?;

    Ok(AddNoteResult { file_path, note_type, edges_written: summary.edges_written })
}

/// Walks the repo's resolved notes folder and ingests every note found —
/// `classifier`/`matcher` are caller-chosen (heuristic by default; an
/// LLM-assisted variant for callers willing to pay for it, e.g.
/// `agentops-cli`'s `--llm-classify`/`--llm-match` flags) so this one
/// function serves both the cheap default path and the opt-in richer one,
/// rather than the two paths diverging into separate implementations.
pub fn ingest_notes_dir(repo_path: &Path, notes_path_override: Option<&Path>, classifier: &dyn agentops_notes::NoteClassifier, matcher: &dyn agentops_notes::SymbolMatcher) -> Result<agentops_notes::IngestSummary> {
    let store = SqliteGraphStore::open(&graph_db_path(repo_path))?;
    let repo = repo_name(repo_path);
    let notes_dir = agentops_notes::resolve_notes_path(repo_path, notes_path_override);
    let notes = agentops_notes::walk_vault(&notes_dir, classifier)?;
    agentops_notes::ingest_vault(&store, &repo, &notes, matcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_note_writes_a_file_and_ingests_it_into_the_graph() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        crate::scan::scan_and_persist(dir.path()).unwrap();

        let result = add_note(dir.path(), "Token bug", "verify_token has a known workaround for a bug.", None, &[], None).unwrap();
        assert_eq!(result.note_type, agentops_notes::NoteType::Gotcha, "gotcha-shaped body must be auto-classified");
        assert!(result.file_path.exists());

        let store = SqliteGraphStore::open(&graph_db_path(dir.path())).unwrap();
        let repo = repo_name(dir.path());
        let gotchas = agentops_graph::GraphStore::nodes_by_kind(&store, &repo, agentops_graph::NodeKind::Gotcha).unwrap();
        assert_eq!(gotchas.len(), 1);
        assert_eq!(gotchas[0].name.as_deref(), Some("Token bug"));
    }

    #[test]
    fn add_note_with_explicit_note_type_skips_classification() {
        let dir = tempfile::tempdir().unwrap();
        crate::scan::scan_and_persist(dir.path()).unwrap();

        let result = add_note(dir.path(), "Some knowledge", "Plain informational text, nothing gotcha- or decision-shaped.", Some(agentops_notes::NoteType::Knowledge), &[], None).unwrap();
        assert_eq!(result.note_type, agentops_notes::NoteType::Knowledge);
    }

    #[test]
    fn ingest_notes_dir_picks_up_a_hand_written_note() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        crate::scan::scan_and_persist(dir.path()).unwrap();

        let notes_dir = dir.path().join(".agentops").join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("hand.md"), "---\ntitle: \"Hand-written\"\ntype: knowledge\n---\n\nSome content.\n").unwrap();

        let classifier = agentops_notes::HeuristicClassifier;
        let matcher = agentops_notes::WordBoundaryMatcher::default();
        let summary = ingest_notes_dir(dir.path(), None, &classifier, &matcher).unwrap();
        assert_eq!(summary.notes_written, 1);
    }
}
