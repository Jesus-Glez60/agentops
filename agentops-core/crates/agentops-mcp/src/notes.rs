//! Shared "write/ingest a note" use cases — used by both the MCP `add_note`/
//! `ingest_notes` tool handlers (`tools.rs`) and `agentops-cli`'s `note`/
//! `ingest-notes` commands. Mirrors `scan.rs`'s role for `scan_and_persist`:
//! one implementation, multiple thin driving adapters, so the CLI and the
//! MCP tool can never drift apart on what "adding a note" actually does.

use std::path::{Path, PathBuf};

use agentops_embeddings::Embedder;
use agentops_graph::NodeKind;
use anyhow::Result;

use crate::scan::repo_name;

pub struct AddNoteResult {
    pub file_path: PathBuf,
    pub note_type: agentops_notes::NoteType,
    pub edges_written: usize,
    pub edges_reinforced: usize,
}

/// Writes a new note to the repo's resolved notes folder and ingests it
/// into the graph in one call. `note_type` of `None` means "classify it" —
/// callers that already know the type (an explicit CLI flag, or a
/// caller-supplied `note_type` string) pass `Some(..)`, including
/// `Some(NoteType::Knowledge)` to skip classification for an explicit
/// "knowledge" request rather than treating it as ambiguous.
///
/// `with_embeddings`: natural-language note text is exactly what dense
/// embeddings are best at, so this is worth offering here too (not just for
/// scan's symbols) — the note's own node is looked up via `find_node` right
/// after ingestion (this function always processes exactly one note, unlike
/// `ingest_notes_dir`'s bulk path below, so there's always exactly one id to
/// attach an embedding to).
pub fn add_note(repo_path: &Path, title: &str, body: &str, note_type: Option<agentops_notes::NoteType>, tags: &[String], notes_path_override: Option<&Path>, with_embeddings: bool) -> Result<AddNoteResult> {
    let store = crate::store::open_store(repo_path)?;
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
    // YAML double-quoted scalar escaping -- backslash first, then quote, so
    // an embedded `\` doesn't get double-escaped by the quote-escaping step.
    // Without this, a title containing a literal `"` (or `\`) produces
    // frontmatter `walk_vault`'s `serde_saphyr::from_str` parses back to a
    // *different*, truncated/mangled title than what was written -- the
    // `this_note.title == title` check just below then finds nothing and
    // fails with "wrote note but failed to re-parse it", even though the
    // file itself was written and the note isn't actually lost. Caught live
    // via a real title containing `#[serde(tag = \"kind\")]`.
    let title_escaped = title.replace('\\', "\\\\").replace('"', "\\\"");
    let content = format!("---\ntitle: \"{title_escaped}\"\ntype: {type_str}\n{tags_yaml}---\n\n{body}\n");
    std::fs::write(&file_path, &content)?;

    let notes = agentops_notes::walk_vault(&notes_dir, &classifier)?;
    let this_note = notes.into_iter().find(|n| n.title == title).ok_or_else(|| anyhow::anyhow!("wrote note but failed to re-parse it"))?;
    let summary = agentops_notes::ingest_vault(store.as_ref(), &repo, std::slice::from_ref(&this_note), &matcher, true)?;

    if with_embeddings {
        let node_kind = note_type.node_kind();
        let vault_path = format!("vault:{}", this_note.source_path.to_string_lossy());
        if let Some(node) = store.find_node(&repo, node_kind, Some(&vault_path), Some(title), None)? {
            let embedding = agentops_embeddings::LocalEmbedder.embed(body)?;
            store.set_embedding(&repo, node.id, &embedding)?;
        }
    }

    Ok(AddNoteResult { file_path, note_type, edges_written: summary.edges_written, edges_reinforced: summary.edges_reinforced })
}

/// Walks the repo's resolved notes folder and ingests every note found —
/// `classifier`/`matcher` are caller-chosen (heuristic by default; an
/// LLM-assisted variant for callers willing to pay for it, e.g.
/// `agentops-cli`'s `--llm-classify`/`--llm-match` flags) so this one
/// function serves both the cheap default path and the opt-in richer one,
/// rather than the two paths diverging into separate implementations.
///
/// `with_embeddings`: `agentops_notes::ingest_vault`'s `IngestSummary`
/// never exposes individual node ids (only counts), so there's no id to
/// hand `set_embedding` straight out of the bulk `ingest_vault` call the
/// way `add_note`'s single-note path can. Simpler than diffing "which ones
/// are actually new": re-embed every gotcha/decision/note node currently in
/// the repo after ingestion completes, each time this is called with
/// `with_embeddings: true` — safe to re-run (`set_embedding` replaces), at
/// the cost of re-embedding unchanged notes on every bulk run.
pub fn ingest_notes_dir(repo_path: &Path, notes_path_override: Option<&Path>, classifier: &dyn agentops_notes::NoteClassifier, matcher: &dyn agentops_notes::SymbolMatcher, with_embeddings: bool) -> Result<agentops_notes::IngestSummary> {
    let store = crate::store::open_store(repo_path)?;
    let repo = repo_name(repo_path);
    let notes_dir = agentops_notes::resolve_notes_path(repo_path, notes_path_override);
    let notes = agentops_notes::walk_vault(&notes_dir, classifier)?;
    let summary = agentops_notes::ingest_vault(store.as_ref(), &repo, &notes, matcher, true)?;

    if with_embeddings {
        for kind in [NodeKind::Gotcha, NodeKind::Decision, NodeKind::Note] {
            for node in store.nodes_by_kind(&repo, kind)? {
                if let Some(content) = &node.content {
                    let embedding = agentops_embeddings::LocalEmbedder.embed(content)?;
                    store.set_embedding(&repo, node.id, &embedding)?;
                }
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use agentops_graph::{GraphStore, SqliteGraphStore};

    use super::*;
    use crate::scan::graph_db_path;

    #[test]
    fn add_note_writes_a_file_and_ingests_it_into_the_graph() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let result = add_note(dir.path(), "Token bug", "verify_token has a known workaround for a bug.", None, &[], None, false).unwrap();
        assert_eq!(result.note_type, agentops_notes::NoteType::Gotcha, "gotcha-shaped body must be auto-classified");
        assert!(result.file_path.exists());

        let store = SqliteGraphStore::open(&graph_db_path(dir.path())).unwrap();
        let repo = repo_name(dir.path());
        let gotchas = agentops_graph::GraphStore::nodes_by_kind(&store, &repo, agentops_graph::NodeKind::Gotcha).unwrap();
        assert_eq!(gotchas.len(), 1);
        assert_eq!(gotchas[0].name.as_deref(), Some("Token bug"));
    }

    #[test]
    fn add_note_with_a_title_containing_a_literal_quote_and_backslash_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let title = r#"serde's tag = \"kind\" fails"#;
        let result = add_note(dir.path(), title, "Body text, nothing special.", Some(agentops_notes::NoteType::Knowledge), &[], None, false).unwrap();

        let store = SqliteGraphStore::open(&graph_db_path(dir.path())).unwrap();
        let repo = repo_name(dir.path());
        let notes = agentops_graph::GraphStore::nodes_by_kind(&store, &repo, agentops_graph::NodeKind::Note).unwrap();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(notes[0].name.as_deref(), Some(title), "the re-parsed title must match the original exactly, not a truncated/mangled one");
        assert!(result.file_path.exists());
    }

    #[test]
    fn add_note_with_explicit_note_type_skips_classification() {
        let dir = tempfile::tempdir().unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let result = add_note(dir.path(), "Some knowledge", "Plain informational text, nothing gotcha- or decision-shaped.", Some(agentops_notes::NoteType::Knowledge), &[], None, false).unwrap();
        assert_eq!(result.note_type, agentops_notes::NoteType::Knowledge);
    }

    #[test]
    fn ingest_notes_dir_picks_up_a_hand_written_note() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token():\n    pass\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        let notes_dir = dir.path().join(".agentops").join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("hand.md"), "---\ntitle: \"Hand-written\"\ntype: knowledge\n---\n\nSome content.\n").unwrap();

        let classifier = agentops_notes::HeuristicClassifier;
        let matcher = agentops_notes::WordBoundaryMatcher::default();
        let summary = ingest_notes_dir(dir.path(), None, &classifier, &matcher, false).unwrap();
        assert_eq!(summary.notes_written, 1);
    }

    #[test]
    fn add_note_with_embeddings_makes_it_findable_via_search_similar() {
        let dir = tempfile::tempdir().unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();

        add_note(dir.path(), "Timing attack risk", "verify_token has a known issue: a plain equality check, vulnerable to timing attacks.", None, &[], None, true).unwrap();

        let store = SqliteGraphStore::open(&graph_db_path(dir.path())).unwrap();
        let repo = repo_name(dir.path());
        let query = agentops_embeddings::LocalEmbedder.embed("comparison is not constant-time, timing side channel").unwrap();
        let hits = store.search_similar(&repo, &query, 5, Some(NodeKind::Gotcha)).unwrap();
        assert!(hits.iter().any(|(n, _)| n.name.as_deref() == Some("Timing attack risk")), "found: {hits:?}");
    }
}
