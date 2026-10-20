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
/// A note title's file name stem: lowercase, every run of non-alphanumeric
/// characters one `-`. A note lives at `vault:<slug>.md`, so the slug is
/// half its natural key (the title is the other half).
pub fn slug(title: &str) -> String {
    let dashed = title.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>();
    dashed.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-")
}

/// Renames a gotcha, decision or note in place: a new title and the
/// `vault:<slug>.md` path that goes with it, keeping its id, edges, tags,
/// curation and vector. The vector stays valid because notes are embedded
/// from their body alone (`add_note_with`, `link_similar_backfill`), never
/// their title; imported memories carry their description in the body.
///
/// The note's file under `notes_dir` moves too, with its frontmatter title
/// rewritten: a rescan re-ingests every file, and a file left at the old
/// name would bring the old node back. Refused when another node already
/// holds the new key, or a different file already sits at the new path.
/// Returns false when the note already has this title.
pub fn retitle_note(store: &dyn agentops_graph::GraphStore, repo: &str, notes_dir: &Path, node_id: i64, title: &str) -> Result<bool> {
    let title = title.trim();
    anyhow::ensure!(!title.is_empty(), "a note's title can't be empty");
    let node = store.get_node(repo, node_id)?.ok_or_else(|| anyhow::anyhow!("no node {node_id}"))?;
    anyhow::ensure!(agentops_graph::similarity::MEMORY_KINDS.contains(&node.kind), "only gotchas, decisions and notes are retitled, not a {}", node.kind.as_db_str());
    let old_rel = node.path.as_deref().and_then(|p| p.strip_prefix("vault:")).map(std::path::PathBuf::from);
    let new_rel = match old_rel.as_ref().and_then(|r| r.parent()).filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => dir.join(format!("{}.md", slug(title))),
        None => std::path::PathBuf::from(format!("{}.md", slug(title))),
    };
    let new_path = format!("vault:{}", new_rel.to_string_lossy());
    if node.name.as_deref() == Some(title) && node.path.as_deref() == Some(new_path.as_str()) {
        return Ok(false);
    }
    if let Some(holder) = store.find_node(repo, node.kind, Some(&new_path), Some(title), node.container.as_deref())?.filter(|n| n.id != node_id) {
        anyhow::bail!("node {} already has the title {title:?}", holder.id);
    }

    let new_file = notes_dir.join(&new_rel);
    let old_file = old_rel.map(|r| notes_dir.join(r)).filter(|f| f.exists());
    if new_file.exists() && old_file.as_ref() != Some(&new_file) {
        anyhow::bail!("{} already exists", new_file.display());
    }
    if let Some(old_file) = old_file {
        let content = std::fs::read_to_string(&old_file)?;
        std::fs::write(&new_file, agentops_notes::with_frontmatter_title(&content, title))?;
        if old_file != new_file {
            std::fs::remove_file(&old_file)?;
        }
    }
    store.rename_node(repo, node_id, Some(&new_path), title)?;
    Ok(true)
}

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
    let slug = slug(title);
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

/// The similarity backfill (milestone 2, ADR 0051): embeds every gotcha,
/// decision and note that has no vector yet, creates the missing
/// `SimilarTo` edges (never reinforcing existing ones), and returns a plain
/// text report: the candidate-pair cosine histogram the thresholds are tuned
/// from, and how many memories still have no edge of any kind. Shared by
/// `agentops link-similar` and `agentops-server link-similar`, which runs it
/// inside the hosted app with that deployment's own store settings.
pub fn link_similar_backfill(store: &dyn agentops_graph::GraphStore, repo: &str) -> Result<String> {
    use agentops_graph::similarity;
    use std::fmt::Write as _;
    let mut embedded = 0;
    for &kind in similarity::MEMORY_KINDS {
        for node in store.nodes_by_kind(repo, kind)? {
            if let (None, Some(content)) = (store.get_embedding(repo, node.id)?, node.content.as_deref()) {
                store.set_embedding(repo, node.id, &agentops_embeddings::LocalEmbedder.embed(content)?)?;
                embedded += 1;
            }
        }
    }
    let summary = similarity::link_all_similar(store, repo, false)?;
    let mut out = String::new();
    writeln!(out, "repo {repo}: embedded {embedded} memories, created {} SimilarTo edges, dropped {} over the degree cap", summary.edges_created, summary.edges_dropped)?;

    let per_node = 2 * similarity::SIMILAR_K;
    let scores = similarity::candidate_pair_scores(store, repo, per_node)?;
    writeln!(out, "candidate pair cosine ({} pairs, {per_node} nearest per memory):", scores.len())?;
    for bucket in (0..20).rev() {
        let (lo, hi) = (bucket as f32 * 0.05, (bucket + 1) as f32 * 0.05);
        let n = scores.iter().filter(|s| **s >= lo && (**s < hi || bucket == 19)).count();
        if n > 0 {
            let mark = if (lo..hi).contains(&similarity::DUPLICATE_MIN_SCORE) {
                "  <- duplicate threshold in this bucket"
            } else if (lo..hi).contains(&similarity::SIMILAR_MIN_SCORE) {
                "  <- link threshold in this bucket"
            } else {
                ""
            };
            writeln!(out, "  {lo:.2}-{hi:.2}  {n:>5}{mark}")?;
        }
    }
    let (mut total, mut unlinked) = (0, 0);
    for &kind in similarity::MEMORY_KINDS {
        for node in store.nodes_by_kind(repo, kind)? {
            total += 1;
            if store.edges_from(repo, node.id)?.is_empty() && store.edges_to(repo, node.id)?.is_empty() {
                unlinked += 1;
            }
        }
    }
    writeln!(out, "memories with no edges of any kind: {unlinked} of {total}")?;
    Ok(out)
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
    fn retitle_note_moves_the_file_keeps_the_node_and_a_rescan_never_brings_the_old_one_back() {
        let dir = tempfile::tempdir().unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();
        let tags = vec!["imported:memory-md".to_string()];
        let written = add_note(dir.path(), "project-old-slug", "Shipped the thing on Tuesday.", Some(agentops_notes::NoteType::Knowledge), &tags, None, false).unwrap();
        let id = written.node_id.unwrap();
        let other = add_note(dir.path(), "Another note", "Unrelated.", Some(agentops_notes::NoteType::Knowledge), &[], None, false).unwrap().node_id.unwrap();
        let notes_dir = agentops_notes::resolve_notes_path(dir.path(), None);
        let repo = repo_name(dir.path());
        {
            let store = SqliteGraphStore::open(&graph_db_path(dir.path())).unwrap();
            store.add_edge(&repo, id, other, agentops_graph::EdgeRelation::SimilarTo).unwrap();
            assert!(retitle_note(&store, &repo, &notes_dir, id, "Shipped the thing on Tuesday").unwrap());
            assert!(!retitle_note(&store, &repo, &notes_dir, id, "Shipped the thing on Tuesday").unwrap(), "same title again is a no-op");
            let err = retitle_note(&store, &repo, &notes_dir, other, "Shipped the thing on Tuesday").unwrap_err().to_string();
            assert!(err.contains(&format!("node {id}")), "a taken key is refused: {err}");
        }
        assert!(!notes_dir.join("project-old-slug.md").exists());
        let moved = std::fs::read_to_string(notes_dir.join("shipped-the-thing-on-tuesday.md")).unwrap();
        assert!(moved.contains("title: \"Shipped the thing on Tuesday\"") && moved.contains("Shipped the thing on Tuesday."), "{moved}");

        // A full rescan re-ingests every note file: still one node, same id.
        crate::scan::scan_and_persist(dir.path(), false).unwrap();
        let store = SqliteGraphStore::open(&graph_db_path(dir.path())).unwrap();
        let notes = store.nodes_by_kind(&repo, agentops_graph::NodeKind::Note).unwrap();
        assert_eq!(notes.len(), 2, "{notes:?}");
        let renamed = store.get_node(&repo, id).unwrap().unwrap();
        assert_eq!(renamed.name.as_deref(), Some("Shipped the thing on Tuesday"));
        assert_eq!(agentops_notes::tags_of(renamed.content.as_deref().unwrap_or_default()), tags);
        assert!(store.edges_from(&repo, id).unwrap().iter().any(|e| e.dst_id == other));
        // Adding it again under the new title finds it, as a re-import does.
        let again = add_note_with(dir.path(), "Shipped the thing on Tuesday", "Shipped the thing on Tuesday.", Some(agentops_notes::NoteType::Knowledge), &tags, None, true, AddMode::DryRun).unwrap();
        assert!(again.already_exists && again.node_id == Some(id), "re-adding under the new title must find node {id}");
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

    /// The backfill embeds memories that have no vector and links them,
    /// creating edges only; a second run creates nothing.
    #[test]
    fn the_similarity_backfill_embeds_missing_vectors_links_once_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.py"), "def verify_token(t):\n    return t == SECRET\n").unwrap();
        crate::scan::scan_and_persist(dir.path(), false).unwrap();
        add_note(dir.path(), "Timing attack in verify_token", "verify_token compares secrets with plain equality, which leaks timing.", Some(agentops_notes::NoteType::Gotcha), &[], None, false).unwrap();
        add_note(dir.path(), "verify_token leaks timing", "verify_token uses plain equality on secrets, leaking timing information.", Some(agentops_notes::NoteType::Gotcha), &[], None, false).unwrap();
        let store = crate::store::open_store(dir.path()).unwrap();
        let repo = crate::scan::repo_name(dir.path());

        let first = super::link_similar_backfill(store.as_ref(), &repo).unwrap();
        assert!(first.contains("embedded 2 memories") && first.contains("created 2 SimilarTo edges") && first.contains("memories with no edges of any kind: 0 of 2"), "{first}");
        let second = super::link_similar_backfill(store.as_ref(), &repo).unwrap();
        assert!(second.contains("embedded 0 memories") && second.contains("created 0 SimilarTo edges"), "{second}");
    }
}
