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
    /// The note's node: `None` on a dry run, or when the note was a
    /// near-copy of an existing memory and `AddMode::WriteUnlessDuplicate`
    /// left it unwritten.
    pub node_id: Option<i64>,
    /// Whether a file was written and ingested.
    pub written: bool,
    /// A node with this note's natural key (kind, file, title) existed
    /// before the call: writing it again updates that node in place.
    pub already_exists: bool,
    /// The existing memory this one nearly copies (cosine at least
    /// `DUPLICATE_MIN_SCORE`), with its score. Not the note's own node.
    pub duplicate_of: Option<(i64, f32)>,
    /// The memories it is (or would be) linked to by `SimilarTo`.
    pub similar_to: Vec<(i64, f32)>,
}

/// How `add_note_with` treats the note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddMode {
    /// Write and ingest, as `add_note` always has.
    Write,
    /// Write unless it nearly copies an existing memory; then reinforce
    /// that memory's similarity edges instead (the MEMORY.md importer).
    WriteUnlessDuplicate,
    /// Classify and compare only: no file, node, edge or reinforcement.
    DryRun,
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
    add_note_with(repo_path, title, body, note_type, tags, notes_path_override, with_embeddings, AddMode::Write)
}

/// `add_note` with a mode (milestone 2). `DryRun` and
/// `WriteUnlessDuplicate` both need the body's embedding, so they compute
/// it even without `with_embeddings`, and every check runs before the
/// first write.
#[allow(clippy::too_many_arguments)]
pub fn add_note_with(repo_path: &Path, title: &str, body: &str, note_type: Option<agentops_notes::NoteType>, tags: &[String], notes_path_override: Option<&Path>, with_embeddings: bool, mode: AddMode) -> Result<AddNoteResult> {
    use agentops_graph::similarity::{similar_memories, DUPLICATE_MIN_SCORE, SIMILAR_K, SIMILAR_MIN_SCORE};
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
    let slug = title.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>();
    let slug = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    let file_path = notes_dir.join(format!("{slug}.md"));

    // Everything below up to the first write only reads. The note's own
    // node (same kind, file and title) is never its own duplicate: writing
    // it again is an update, which is what makes a re-import idempotent.
    let existing = store.find_node(&repo, note_type.node_kind(), Some(&format!("vault:{slug}.md")), Some(title), None)?.map(|n| n.id);
    let embedding = if with_embeddings || mode != AddMode::Write { Some(agentops_embeddings::LocalEmbedder.embed(body)?) } else { None };
    let (duplicate_of, similar_to) = match &embedding {
        Some(embedding) if mode != AddMode::Write => {
            let near: Vec<(i64, f32)> = similar_memories(store.as_ref(), &repo, embedding, existing, SIMILAR_K)?.into_iter().map(|(n, score)| (n.id, score)).collect();
            (near.first().copied().filter(|(_, score)| *score >= DUPLICATE_MIN_SCORE), near.into_iter().filter(|(_, score)| *score >= SIMILAR_MIN_SCORE).collect())
        }
        _ => (None, Vec::new()),
    };
    let unwritten = |similar_to: Vec<(i64, f32)>| AddNoteResult { file_path: file_path.clone(), note_type, edges_written: 0, edges_reinforced: 0, node_id: existing, written: false, already_exists: existing.is_some(), duplicate_of, similar_to };
    if mode == AddMode::DryRun {
        return Ok(unwritten(similar_to));
    }
    if let (AddMode::WriteUnlessDuplicate, Some((duplicate, _))) = (mode, duplicate_of) {
        agentops_graph::similarity::link_similar(store.as_ref(), &repo, duplicate)?;
        return Ok(AddNoteResult { node_id: None, ..unwritten(similar_to) });
    }

    std::fs::create_dir_all(&notes_dir)?;

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

    let vault_path = format!("vault:{}", this_note.source_path.to_string_lossy());
    let node_id = store.find_node(&repo, note_type.node_kind(), Some(&vault_path), Some(title), None)?.map(|n| n.id);
    let mut similar_to = similar_to;
    if let (Some(embedding), Some(id)) = (&embedding, node_id) {
        store.set_embedding(&repo, id, embedding)?;
        similar_to = agentops_graph::similarity::link_similar(store.as_ref(), &repo, id)?.linked;
    }

    Ok(AddNoteResult { file_path, note_type, edges_written: summary.edges_written, edges_reinforced: summary.edges_reinforced, node_id, written: true, already_exists: existing.is_some(), duplicate_of, similar_to })
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
        agentops_graph::similarity::link_all_similar(store.as_ref(), &repo, true)?;
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

    /// Milestone 2: a dry run writes nothing, a near-copy is skipped (the
    /// existing memory is reinforced), and writing the same note again
    /// updates it in place, which is what makes the importer idempotent.
    #[test]
    fn add_note_modes_dry_run_skip_a_near_copy_and_update_in_place() {
        use super::{add_note_with, AddMode};
        use agentops_notes::NoteType;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token(t):\n    return t == SECRET\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();
        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());
        let count = || store.nodes_by_kind(&repo, agentops_graph::NodeKind::Gotcha).unwrap().len();
        let body = "verify_token compares secrets with plain equality, which leaks timing; use a constant-time compare.";

        let first = add_note_with(dir.path(), "Timing attack in verify_token", body, Some(NoteType::Gotcha), &[], None, true, AddMode::Write).unwrap();
        let first_id = first.node_id.unwrap();
        assert!(first.written && !first.already_exists);

        let dry = add_note_with(dir.path(), "Same thing, other words", body, Some(NoteType::Gotcha), &[], None, true, AddMode::DryRun).unwrap();
        assert!(!dry.written && dry.node_id.is_none());
        assert_eq!(dry.duplicate_of.map(|(id, _)| id), Some(first_id));
        assert!(!dry.file_path.exists(), "a dry run writes no file");
        assert_eq!(count(), 1);

        let skipped = add_note_with(dir.path(), "Same thing, other words", body, Some(NoteType::Gotcha), &[], None, true, AddMode::WriteUnlessDuplicate).unwrap();
        assert!(!skipped.written && skipped.node_id.is_none() && !skipped.file_path.exists());
        assert_eq!(count(), 1);

        let again = add_note_with(dir.path(), "Timing attack in verify_token", body, Some(NoteType::Gotcha), &[], None, true, AddMode::WriteUnlessDuplicate).unwrap();
        assert!(again.written && again.already_exists && again.duplicate_of.is_none(), "its own node is never its duplicate");
        assert_eq!((again.node_id, count()), (Some(first_id), 1));

        let other = add_note_with(dir.path(), "Unrelated decision", "We deploy on push to main through a CI gate on the server.", Some(NoteType::Gotcha), &[], None, true, AddMode::WriteUnlessDuplicate).unwrap();
        assert!(other.written && other.duplicate_of.is_none());
        assert_eq!(count(), 2);
    }
}
