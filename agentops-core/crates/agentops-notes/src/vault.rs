//! Recursive markdown vault/notes-folder ingestion, symbol-matched into the
//! same repo-scoped graph gotchas/decisions already live in — not docbrain
//! (see the plan's "Architectural call" section: docbrain's `DocNode` is
//! keyed to a versioned *library*, has no symbol-tying mechanism at all, and
//! its `Visibility` is org-scoped, not repo-scoped; none of that fits "my
//! own repo's project notes").
//!
//! Formalizes what a demo session did by hand with a throwaway Python
//! script: read a vault's markdown notes, regex-match each one's body
//! against real symbol names, and connect them via `agentops note
//! --affects`. This module is the real, repeatable version of that.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use agentops_graph::{EdgeRelation, GraphStore, NewNode, NodeKind};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

/// The vault's own frontmatter `type:` convention, confirmed against the
/// real 61-note CurrentYachts corpus. `Gotcha`/`Decision` reuse the existing
/// `NodeKind` variants directly; `Knowledge`/`Context` (and anything
/// untyped) become `NodeKind::Note`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteType {
    Gotcha,
    Decision,
    Knowledge,
    Context,
}

impl NoteType {
    pub fn node_kind(self) -> NodeKind {
        match self {
            NoteType::Gotcha => NodeKind::Gotcha,
            NoteType::Decision => NodeKind::Decision,
            NoteType::Knowledge | NoteType::Context => NodeKind::Note,
        }
    }

    /// Infers a type from frontmatter text, falling back to the note's
    /// parent folder name (`.../gotchas/foo.md` implies `gotcha` even with
    /// no frontmatter at all) — a free, real signal the original demo
    /// script didn't use. Defaults to `Knowledge` (-> `NodeKind::Note`) when
    /// neither source gives an answer, rather than guessing `Gotcha`.
    fn infer(frontmatter_type: Option<&str>, parent_folder: Option<&str>) -> Self {
        let from_str = |s: &str| match s.to_lowercase().as_str() {
            "gotcha" | "gotchas" => Some(NoteType::Gotcha),
            "decision" | "decisions" => Some(NoteType::Decision),
            "knowledge" => Some(NoteType::Knowledge),
            "context" | "contexts" => Some(NoteType::Context),
            _ => None,
        };
        frontmatter_type.and_then(&from_str).or_else(|| parent_folder.and_then(&from_str)).unwrap_or(NoteType::Knowledge)
    }
}

#[derive(Debug, Deserialize, Default)]
struct Frontmatter {
    title: Option<String>,
    #[serde(rename = "type")]
    note_type: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// One parsed vault note, ready for symbol matching + graph insertion.
#[derive(Debug, Clone)]
pub struct VaultNote {
    pub title: String,
    pub note_type: NoteType,
    pub tags: Vec<String>,
    pub body: String,
    /// Relative to the vault root — used as the note's graph `path` (with a
    /// `vault:` prefix, see `ingest_vault`) for idempotent re-ingestion, and
    /// for provenance.
    pub source_path: PathBuf,
}

/// Splits a markdown file's leading `---`-delimited YAML frontmatter (the
/// standard Obsidian convention) from its body. Returns `(frontmatter,
/// body)` — `frontmatter` is `None` if the file doesn't start with `---`.
fn split_frontmatter(content: &str) -> (Option<Frontmatter>, &str) {
    let Some(rest) = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n")) else {
        return (None, content);
    };
    let Some(end) = rest.find("\n---") else {
        return (None, content);
    };
    let yaml = &rest[..end];
    // `end` points at the `\n` *before* the closing `---` -- skip that
    // newline plus the three dashes themselves, then find the newline
    // *after* the closing delimiter to locate where the body actually
    // starts. (A prior version searched for a newline starting at `end`
    // itself, which is the leading `\n` of `\n---` -- position zero of that
    // search -- so it always returned `end + 1`, landing body_start right
    // on the closing `---` line instead of past it.)
    let after_delimiter = end + "\n---".len();
    let body_start = rest[after_delimiter..].find('\n').map(|i| after_delimiter + i + 1).unwrap_or(rest.len());
    let body = &rest[body_start..];

    match serde_saphyr::from_str::<Frontmatter>(yaml) {
        Ok(fm) => (Some(fm), body),
        Err(_) => (None, content), // malformed frontmatter -- treat the whole file as body rather than failing ingestion
    }
}

/// Recursively walks `root` for `*.md` files and parses each into a
/// `VaultNote`. Default chunking is whole-file-as-one-node (matches the real
/// 61-note corpus's shape: short, single-topic files) — heading-level
/// chunking for the rarer long-form note is an explicitly deferred follow-on
/// (see the plan), not built here.
pub fn walk_vault(root: &Path) -> Result<Vec<VaultNote>> {
    let mut notes = Vec::new();
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let content = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let (frontmatter, body) = split_frontmatter(&content);
        let parent_folder = path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());

        let fm = frontmatter.unwrap_or_default();
        let note_type = NoteType::infer(fm.note_type.as_deref(), parent_folder);
        let title = fm
            .title
            .filter(|t| !t.is_empty())
            .or_else(|| first_h1(body))
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "(untitled)".to_string()));

        let source_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        notes.push(VaultNote { title, note_type, tags: fm.tags, body: body.trim().to_string(), source_path });
    }
    Ok(notes)
}

fn first_h1(body: &str) -> Option<String> {
    body.lines().find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
}

/// Cheap, no-network default symbol matcher: word-boundary regex over
/// `note_body`, ranked by name length descending (a longer, more specific
/// match wins over a short substring collision — e.g. `enrichZohoContact`
/// over `Contact`), returning **all** matches above `min_name_len`
/// characters rather than a single guess, since one note can legitimately
/// affect several symbols. Names shorter than `min_name_len` are excluded as
/// a false-positive guard, and names repeated across more than
/// `MAX_NAME_OCCURRENCES` distinct symbols in the repo are excluded
/// entirely, not just length-filtered — confirmed necessary against the real
/// CurrentYachts vault/repo: every Next.js API route file exports functions
/// literally named `GET`/`POST`/`PUT`/`DELETE`/`PATCH` (a framework
/// convention, not a meaningful unique identifier), so a note that merely
/// mentions "a POST request" in ordinary English otherwise matched *every*
/// route handler in the repo — dozens of nonsense edges from one note. A
/// name repeated many times across distinct files is exactly the signal
/// that it's a reserved/convention word, not something a note could
/// specifically mean; a real, deliberately-named symbol like
/// `enrichZohoContact` naturally appears once.
const MAX_NAME_OCCURRENCES: usize = 3;

pub fn match_symbols(store: &dyn GraphStore, repo: &str, note_body: &str, min_name_len: usize) -> Result<Vec<(i64, usize)>> {
    let all_symbols: Vec<agentops_graph::Node> = store.nodes_by_kind(NodeKind::Symbol)?.into_iter().filter(|s| s.repo == repo).collect();

    let mut name_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for s in &all_symbols {
        if let Some(name) = s.name.as_deref() {
            *name_counts.entry(name).or_insert(0) += 1;
        }
    }

    let mut candidates: Vec<&agentops_graph::Node> = all_symbols
        .iter()
        .filter(|s| {
            s.name.as_deref().is_some_and(|n| n.len() >= min_name_len && name_counts.get(n).copied().unwrap_or(0) <= MAX_NAME_OCCURRENCES)
        })
        .collect();
    candidates.sort_by_key(|s| std::cmp::Reverse(s.name.as_deref().map(str::len).unwrap_or(0)));

    let mut matches = Vec::new();
    for symbol in &candidates {
        let name = symbol.name.as_deref().unwrap_or_default();
        let pattern = Regex::new(&format!(r"\b{}\b", regex::escape(name))).context("building word-boundary pattern")?;
        if pattern.is_match(note_body) {
            matches.push((symbol.id, name.len()));
        }
    }
    Ok(matches)
}

/// Connects `note_id` to every id in `targets` via `relation`, skipping any
/// edge that already exists (checked via `edges_from(note_id)` first) so
/// re-running ingestion on an unchanged vault doesn't accumulate duplicate
/// edges the same way `upsert_node` already avoids duplicating nodes. Real,
/// non-trivial responsibility (dedup + loop) — not a bare rename of
/// `store.add_edge`, which is this codebase's own bar for when a store-call
/// wrapper is worth having (see `add_note`).
pub fn connect_many(store: &dyn GraphStore, note_id: i64, targets: &[i64], relation: EdgeRelation) -> Result<usize> {
    let existing: HashSet<i64> = store.edges_from(note_id)?.into_iter().filter(|e| e.relation == relation).map(|e| e.dst_id).collect();
    let mut connected = 0;
    for &target_id in targets {
        if existing.contains(&target_id) {
            continue;
        }
        store.add_edge(note_id, target_id, relation)?;
        connected += 1;
    }
    Ok(connected)
}

/// Picks which candidate symbols (from the cheap default's already-narrowed
/// shortlist) a note is actually about — the seam `--llm-match` plugs an
/// LLM-assisted re-ranker into, without `agentops-notes` itself gaining a
/// network dependency. `agentops_notes::match_symbols` (this crate) is the
/// default, no-network implementation; `agentops-llm`-backed ones live in
/// whichever crate already depends on both (the CLI/MCP layer).
pub trait SymbolMatcher {
    fn match_symbols(&self, store: &dyn GraphStore, repo: &str, note_body: &str) -> Result<Vec<i64>>;
}

/// The cheap default matcher, as a `SymbolMatcher` — used when no
/// `--llm-match` flag/LLM config is given.
pub struct WordBoundaryMatcher {
    pub min_name_len: usize,
}

impl Default for WordBoundaryMatcher {
    fn default() -> Self {
        Self { min_name_len: 4 }
    }
}

impl SymbolMatcher for WordBoundaryMatcher {
    fn match_symbols(&self, store: &dyn GraphStore, repo: &str, note_body: &str) -> Result<Vec<i64>> {
        Ok(match_symbols(store, repo, note_body, self.min_name_len)?.into_iter().map(|(id, _)| id).collect())
    }
}

#[derive(Debug, Default)]
pub struct IngestSummary {
    pub notes_seen: usize,
    pub notes_written: usize,
    pub edges_written: usize,
}

/// Ingests `notes` into `repo`'s graph: one node per `VaultNote`
/// (`Gotcha`/`Decision`/`Note` per its inferred type), matched to symbols
/// via `matcher` and connected via `Affects` edges (using
/// `AffectsTarget::NodeId`-equivalent resolution — `connect_many` takes
/// already-resolved ids directly, never re-resolving by name, so this path
/// never touches `resolve_symbol_by_name`'s ambiguity/scoping bug).
///
/// Idempotent: a note's `path` is set to `vault:{source_path}` (the `vault:`
/// prefix disambiguates it from a scanned code file's path in the same
/// `nodes` table, which has no other type discriminator beyond `kind`) and
/// its `name` to its title, so `find_node`'s natural key lets `upsert_node`
/// update an unchanged vault's notes in place on re-ingestion instead of
/// duplicating them.
pub fn ingest_vault(store: &dyn GraphStore, repo: &str, notes: &[VaultNote], matcher: &dyn SymbolMatcher) -> Result<IngestSummary> {
    let mut summary = IngestSummary { notes_seen: notes.len(), ..Default::default() };

    for note in notes {
        let vault_path = format!("vault:{}", note.source_path.to_string_lossy());
        let note_id = agentops_graph::upsert_node(
            store,
            NewNode {
                kind: note.note_type.node_kind(),
                repo: repo.to_string(),
                path: Some(vault_path),
                name: Some(note.title.clone()),
                start_line: None,
                end_line: None,
                content: Some(note.body.clone()),
            },
        )?;
        summary.notes_written += 1;

        let targets = matcher.match_symbols(store, repo, &note.body)?;
        summary.edges_written += connect_many(store, note_id, &targets, EdgeRelation::Affects)?;
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::SqliteGraphStore;

    fn write_note(dir: &Path, rel_path: &str, content: &str) {
        let full = dir.join(rel_path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }

    #[test]
    fn walk_vault_parses_frontmatter_title_type_and_tags() {
        let dir = tempfile::tempdir().unwrap();
        write_note(
            dir.path(),
            "gotchas/zoho.md",
            "---\ntitle: \"Zoho duplicate contact\"\ntype: gotcha\ntags: [zoho, crm]\n---\n\nBody text here.\n",
        );

        let notes = walk_vault(dir.path()).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Zoho duplicate contact");
        assert_eq!(notes[0].note_type, NoteType::Gotcha);
        assert_eq!(notes[0].tags, vec!["zoho", "crm"]);
        assert_eq!(notes[0].body, "Body text here.");
    }

    #[test]
    fn walk_vault_infers_type_from_parent_folder_when_frontmatter_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "decisions/use-sqlite.md", "No frontmatter, just prose.\n");

        let notes = walk_vault(dir.path()).unwrap();
        assert_eq!(notes[0].note_type, NoteType::Decision);
        assert_eq!(notes[0].title, "use-sqlite", "falls back to the filename stem when there's no frontmatter title or H1");
    }

    #[test]
    fn walk_vault_falls_back_to_first_h1_for_title_when_frontmatter_has_none() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "knowledge/misc.md", "---\ntype: knowledge\n---\n\n# The Real Title\n\nBody.\n");

        let notes = walk_vault(dir.path()).unwrap();
        assert_eq!(notes[0].title, "The Real Title");
        assert_eq!(notes[0].note_type, NoteType::Knowledge);
    }

    #[test]
    fn walk_vault_only_reads_markdown_files_recursively() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "a/b/c/deep.md", "deep note\n");
        write_note(dir.path(), "notes.txt", "not markdown, must be ignored\n");

        let notes = walk_vault(dir.path()).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].source_path.to_string_lossy().contains("deep.md"));
    }

    #[test]
    fn match_symbols_prefers_longer_more_specific_names_and_respects_min_len() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        agentops_graph::upsert_node(
            &store,
            NewNode { kind: NodeKind::Symbol, repo: "demo".into(), path: Some("a.rs".into()), name: Some("enrichZohoContact".into()), start_line: Some(1), end_line: Some(2), content: Some("..".into()) },
        )
        .unwrap();
        agentops_graph::upsert_node(
            &store,
            NewNode { kind: NodeKind::Symbol, repo: "demo".into(), path: Some("b.rs".into()), name: Some("session".into()), start_line: Some(1), end_line: Some(2), content: Some("..".into()) },
        )
        .unwrap();
        // Below min_name_len (4) -- must be excluded even though it appears in the body.
        agentops_graph::upsert_node(
            &store,
            NewNode { kind: NodeKind::Symbol, repo: "demo".into(), path: Some("c.rs".into()), name: Some("id".into()), start_line: Some(1), end_line: Some(2), content: Some("..".into()) },
        )
        .unwrap();

        let matches = match_symbols(&store, "demo", "Calls enrichZohoContact during session id lookup.", 4).unwrap();
        let names: Vec<_> = matches.iter().map(|(_, len)| *len).collect();
        assert_eq!(matches.len(), 2, "the 2-char name below min_name_len must be excluded: {matches:?}");
        assert!(names.contains(&"enrichZohoContact".len()));
        assert!(names.contains(&"session".len()));
    }

    #[test]
    fn match_symbols_excludes_names_repeated_across_many_files_as_framework_noise() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        // Same name in 4 different files -- a Next.js-style route handler
        // convention, not a meaningful unique reference.
        for path in ["a/route.js", "b/route.js", "c/route.js", "d/route.js"] {
            agentops_graph::upsert_node(
                &store,
                NewNode { kind: NodeKind::Symbol, repo: "demo".into(), path: Some(path.into()), name: Some("POST".into()), start_line: Some(1), end_line: Some(2), content: Some("..".into()) },
            )
            .unwrap();
        }
        agentops_graph::upsert_node(
            &store,
            NewNode { kind: NodeKind::Symbol, repo: "demo".into(), path: Some("zoho.js".into()), name: Some("enrichZohoContact".into()), start_line: Some(1), end_line: Some(2), content: Some("..".into()) },
        )
        .unwrap();

        let matches = match_symbols(&store, "demo", "Sends a POST request that calls enrichZohoContact internally.", 4).unwrap();
        assert_eq!(matches.len(), 1, "POST must be excluded as framework noise, leaving only the real unique match: {matches:?}");
    }

    #[test]
    fn match_symbols_is_repo_scoped() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        agentops_graph::upsert_node(
            &store,
            NewNode { kind: NodeKind::Symbol, repo: "other-repo".into(), path: Some("a.rs".into()), name: Some("verify_token".into()), start_line: Some(1), end_line: Some(2), content: Some("..".into()) },
        )
        .unwrap();

        let matches = match_symbols(&store, "demo", "Calls verify_token here.", 4).unwrap();
        assert!(matches.is_empty(), "a same-named symbol in a different repo must not match");
    }

    #[test]
    fn connect_many_dedupes_against_already_existing_edges() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let note_id = store.add_node(NewNode { kind: NodeKind::Note, repo: "demo".into(), path: None, name: Some("n".into()), start_line: None, end_line: None, content: Some("x".into()) }).unwrap();
        let target_id =
            agentops_graph::upsert_node(&store, NewNode { kind: NodeKind::Symbol, repo: "demo".into(), path: Some("a.rs".into()), name: Some("f".into()), start_line: Some(1), end_line: Some(2), content: Some("..".into()) }).unwrap();

        let first = connect_many(&store, note_id, &[target_id], EdgeRelation::Affects).unwrap();
        let second = connect_many(&store, note_id, &[target_id], EdgeRelation::Affects).unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 0, "re-connecting the same target must not duplicate the edge");
        assert_eq!(store.edges_from(note_id).unwrap().len(), 1);
    }

    #[test]
    fn ingest_vault_creates_typed_nodes_and_affects_edges_via_the_given_matcher() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        agentops_graph::upsert_node(
            &store,
            NewNode { kind: NodeKind::Symbol, repo: "demo".into(), path: Some("auth.rs".into()), name: Some("verify_token".into()), start_line: Some(1), end_line: Some(2), content: Some("..".into()) },
        )
        .unwrap();

        let notes =
            vec![VaultNote { title: "token bug".into(), note_type: NoteType::Gotcha, tags: vec![], body: "verify_token has an off-by-one bug.".into(), source_path: PathBuf::from("gotchas/token.md") }];

        let matcher = WordBoundaryMatcher::default();
        let summary = ingest_vault(&store, "demo", &notes, &matcher).unwrap();

        assert_eq!(summary.notes_written, 1);
        assert_eq!(summary.edges_written, 1);
        let gotchas = store.nodes_by_kind(NodeKind::Gotcha).unwrap();
        assert_eq!(gotchas.len(), 1);
        assert_eq!(gotchas[0].name.as_deref(), Some("token bug"));
    }

    #[test]
    fn re_ingesting_an_unchanged_vault_updates_in_place_instead_of_duplicating() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let notes = vec![VaultNote { title: "n".into(), note_type: NoteType::Knowledge, tags: vec![], body: "body".into(), source_path: PathBuf::from("knowledge/n.md") }];
        let matcher = WordBoundaryMatcher::default();

        ingest_vault(&store, "demo", &notes, &matcher).unwrap();
        ingest_vault(&store, "demo", &notes, &matcher).unwrap();

        assert_eq!(store.nodes_by_kind(NodeKind::Note).unwrap().len(), 1, "re-ingesting the same vault must not duplicate notes");
    }
}
