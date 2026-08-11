//! Recursive markdown notes-folder ingestion, symbol-matched into the same
//! repo-scoped graph gotchas/decisions live in. Deliberately generalized:
//! `main` assumed an organized, Obsidian-style vault (frontmatter `type:` /
//! folder-name classification only) — most users won't have this, so a
//! `NoteClassifier` port handles freeform, unorganized notes too (a
//! README, a scattered NOTES.md) instead of silently bucketing everything
//! that lacks a signal into `Knowledge`. A real vault and an unorganized
//! notes folder are the *same* code path here — both are just a directory
//! of Markdown files, some with frontmatter and some without.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agentops_graph::{upsert_node, EdgeRelation, GraphStore, NewNode, NodeKind};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;

/// The notes' own frontmatter `type:` convention. `Gotcha`/`Decision` reuse
/// the existing `NodeKind` variants directly; `Knowledge`/`Context` (and
/// anything untyped and unclassifiable) become `NodeKind::Note`.
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

    fn from_str_hint(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "gotcha" | "gotchas" => Some(NoteType::Gotcha),
            "decision" | "decisions" => Some(NoteType::Decision),
            "knowledge" => Some(NoteType::Knowledge),
            "context" | "contexts" => Some(NoteType::Context),
            _ => None,
        }
    }

    /// Infers a type: frontmatter `type:` → parent folder name → the given
    /// `NoteClassifier` (heuristic or LLM-assisted) when neither gives a
    /// signal at all. Only the last step can genuinely guess from content —
    /// the first two are exact, structural signals.
    fn infer(frontmatter_type: Option<&str>, parent_folder: Option<&str>, body: &str, classifier: &dyn NoteClassifier) -> Result<Self> {
        if let Some(t) = frontmatter_type.and_then(Self::from_str_hint) {
            return Ok(t);
        }
        if let Some(t) = parent_folder.and_then(Self::from_str_hint) {
            return Ok(t);
        }
        classifier.classify(body)
    }
}

/// The note-type classification port — the actual generalization this
/// module exists for. `HeuristicClassifier` (this crate) is the default,
/// no-network adapter; `LlmAssistedClassifier` (`agentops-llm`) is a second
/// adapter for genuinely ambiguous content.
pub trait NoteClassifier {
    fn classify(&self, note_body: &str) -> Result<NoteType>;
}

/// Gotcha-shaped vs. decision-shaped language, scored by keyword presence.
/// `confident` is `false` when there's a tie (including "no keywords at
/// all", scored as a 0-0 tie) — a signal `LlmAssistedClassifier` uses to
/// decide whether a note is worth an API call, without this crate itself
/// needing to know anything about LLMs.
const GOTCHA_KEYWORDS: &[&str] = &["workaround", "doesn't work", "does not work", "known issue", "fails when", "gotcha", "silently", "broken", "bug"];
const DECISION_KEYWORDS: &[&str] = &["decided to", "we chose", "chose to", "instead of", "tradeoff", "trade-off", "rationale"];

/// Scores `body` for gotcha-shaped vs. decision-shaped language. Exposed
/// (not just wrapped by `HeuristicClassifier`) so `LlmAssistedClassifier`
/// can reuse the exact same scoring to decide whether it's worth an API
/// call, without duplicating the keyword lists in `agentops-llm`.
pub fn heuristic_score(body: &str) -> (NoteType, bool) {
    let lower = body.to_lowercase();
    let gotcha_score = GOTCHA_KEYWORDS.iter().filter(|k| lower.contains(*k)).count();
    let decision_score = DECISION_KEYWORDS.iter().filter(|k| lower.contains(*k)).count();

    match gotcha_score.cmp(&decision_score) {
        std::cmp::Ordering::Greater => (NoteType::Gotcha, true),
        std::cmp::Ordering::Less => (NoteType::Decision, true),
        // Tie — including 0-0 (no keywords at all) — is genuinely
        // inconclusive, not confidently "neither".
        std::cmp::Ordering::Equal => (NoteType::Knowledge, false),
    }
}

/// The default, no-network classifier — used whenever frontmatter and
/// folder name both give no signal, and no LLM-assisted classifier is
/// configured (or the heuristic is confident enough not to need one).
pub struct HeuristicClassifier;

impl NoteClassifier for HeuristicClassifier {
    fn classify(&self, note_body: &str) -> Result<NoteType> {
        Ok(heuristic_score(note_body).0)
    }
}

#[derive(Debug, Deserialize, Default)]
struct Frontmatter {
    title: Option<String>,
    #[serde(rename = "type")]
    note_type: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    /// Actually parsed this time — `main`'s doc comments claimed this field
    /// existed but it was never deserialized.
    status: Option<String>,
}

/// One parsed note, ready for symbol matching + graph insertion.
#[derive(Debug, Clone)]
pub struct VaultNote {
    pub title: String,
    pub note_type: NoteType,
    pub tags: Vec<String>,
    pub status: Option<String>,
    pub body: String,
    /// Relative to the notes root — used as the note's graph `path` (with a
    /// `vault:` prefix, see `ingest_vault`) for idempotent re-ingestion.
    pub source_path: PathBuf,
}

/// Splits a markdown file's leading `---`-delimited YAML frontmatter (the
/// standard Obsidian convention) from its body. Returns `(frontmatter,
/// body)` — `frontmatter` is `None` if the file doesn't start with `---`,
/// or if the YAML fails to parse (never fails ingestion over a malformed
/// note; the whole file becomes body text instead).
fn split_frontmatter(content: &str) -> (Option<Frontmatter>, &str) {
    let Some(rest) = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n")) else {
        return (None, content);
    };
    let Some(end) = rest.find("\n---") else {
        return (None, content);
    };
    let yaml = &rest[..end];
    let after_delimiter = end + "\n---".len();
    let body_start = rest[after_delimiter..].find('\n').map(|i| after_delimiter + i + 1).unwrap_or(rest.len());
    let body = &rest[body_start..];

    match serde_saphyr::from_str::<Frontmatter>(yaml) {
        Ok(fm) => (Some(fm), body),
        Err(_) => (None, content),
    }
}

/// Recursively walks `root` for `*.md` files and parses each into a
/// `VaultNote`, classifying its type via frontmatter → folder name →
/// `classifier` in that order. `root` can be a real Obsidian vault or an
/// unorganized `.agentops/notes/` folder — both are just a directory of
/// Markdown files to this function.
pub fn walk_vault(root: &Path, classifier: &dyn NoteClassifier) -> Result<Vec<VaultNote>> {
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
        let note_type = NoteType::infer(fm.note_type.as_deref(), parent_folder, body, classifier)?;
        let title = fm
            .title
            .filter(|t| !t.is_empty())
            .or_else(|| first_h1(body))
            .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "(untitled)".to_string()));

        let source_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        notes.push(VaultNote { title, note_type, tags: fm.tags, status: fm.status, body: body.trim().to_string(), source_path });
    }
    Ok(notes)
}

fn first_h1(body: &str) -> Option<String> {
    body.lines().find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
}

/// Names repeated across more than this many distinct symbols in the repo
/// are excluded entirely as framework-convention noise (e.g. every Next.js
/// API route file exports functions literally named `GET`/`POST`/`PUT` —
/// not a meaningful unique identifier). A real, deliberately-named symbol
/// naturally appears once.
const MAX_NAME_OCCURRENCES: usize = 3;

/// Cheap, no-network default symbol matcher: word-boundary regex over
/// `note_body`, returning **all** matches above `min_name_len` characters
/// (one note can affect several symbols), ranked longest-name-first so a
/// more specific match wins over a short substring collision.
pub fn match_symbols(store: &dyn GraphStore, repo: &str, note_body: &str, min_name_len: usize) -> Result<Vec<(i64, usize)>> {
    let all_symbols = store.nodes_by_kind(repo, NodeKind::Symbol)?;

    let mut name_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for s in &all_symbols {
        if let Some(name) = s.name.as_deref() {
            *name_counts.entry(name).or_insert(0) += 1;
        }
    }

    let mut candidates: Vec<&agentops_graph::Node> =
        all_symbols.iter().filter(|s| s.name.as_deref().is_some_and(|n| n.len() >= min_name_len && name_counts.get(n).copied().unwrap_or(0) <= MAX_NAME_OCCURRENCES)).collect();
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConnectSummary {
    pub added: usize,
    pub reinforced: usize,
}

/// Connects `note_id` to every id in `targets` via `relation`. An edge that
/// already exists is **reinforced** (its weight bumped, see
/// `GraphStore::reinforce_edge`) rather than silently skipped, so a note
/// that keeps matching the same symbol across repeated ingestions (e.g.
/// every rescan) strengthens that relationship over time instead of the
/// second-and-later ingestions being a true no-op.
///
/// `bump_confirmed_at` is forwarded to `reinforce_edge` verbatim — see its
/// doc comment. Must be `false` for the automatic every-scan rematch and
/// `true` only for an explicit, human-initiated reinforcement.
pub fn connect_many(store: &dyn GraphStore, repo: &str, note_id: i64, targets: &[i64], relation: EdgeRelation, bump_confirmed_at: bool) -> Result<ConnectSummary> {
    let existing: HashMap<i64, i64> = store.edges_from(repo, note_id)?.into_iter().filter(|e| e.relation == relation).map(|e| (e.dst_id, e.id)).collect();
    let mut summary = ConnectSummary::default();
    for &target_id in targets {
        match existing.get(&target_id) {
            Some(&edge_id) => {
                store.reinforce_edge(repo, edge_id, bump_confirmed_at)?;
                summary.reinforced += 1;
            }
            None => {
                store.add_edge(repo, note_id, target_id, relation)?;
                summary.added += 1;
            }
        }
    }
    Ok(summary)
}

/// Picks which candidate symbols a note is actually about — the seam an
/// LLM-assisted re-ranker plugs into without `agentops-notes` itself
/// gaining a network dependency.
pub trait SymbolMatcher {
    fn match_symbols(&self, store: &dyn GraphStore, repo: &str, note_body: &str) -> Result<Vec<i64>>;
}

/// The cheap default matcher, as a `SymbolMatcher`.
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
    pub edges_reinforced: usize,
}

/// Ingests `notes` into `repo`'s graph: one node per `VaultNote`
/// (`Gotcha`/`Decision`/`Note` per its inferred type), matched to symbols
/// via `matcher` and connected via `Affects` edges (note → symbol,
/// preserving the direction `agentops-docgen` depends on).
///
/// Idempotent: a note's `path` is `vault:{source_path}` (disambiguates from
/// a scanned code file's path in the same `nodes` table) and its `name` is
/// its title, so `upsert_node`'s natural key updates an unchanged note in
/// place on re-ingestion instead of duplicating it.
///
/// `bump_confirmed_at` — see `connect_many`'s doc comment. Callers: the
/// automatic every-scan rematch (`scan::persist`) must pass `false`;
/// explicit single/import ingestion from `add_note`/`import_notes`
/// (`agentops-mcp/src/notes.rs`) passes `true`.
pub fn ingest_vault(store: &dyn GraphStore, repo: &str, notes: &[VaultNote], matcher: &dyn SymbolMatcher, bump_confirmed_at: bool) -> Result<IngestSummary> {
    let mut summary = IngestSummary { notes_seen: notes.len(), ..Default::default() };

    for note in notes {
        let vault_path = format!("vault:{}", note.source_path.to_string_lossy());
        let note_id = upsert_node(
            store,
            NewNode {
                kind: note.note_type.node_kind(),
                repo: repo.to_string(),
                path: Some(vault_path),
                name: Some(note.title.clone()),
                container: None,
                start_line: None,
                end_line: None,
                content: Some(note.body.clone()),
            },
        )?;
        summary.notes_written += 1;

        let targets = matcher.match_symbols(store, repo, &note.body)?;
        let connected = connect_many(store, repo, note_id, &targets, EdgeRelation::Affects, bump_confirmed_at)?;
        summary.edges_written += connected.added;
        summary.edges_reinforced += connected.reinforced;
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

    fn node(repo: &str, path: &str, name: &str) -> NewNode {
        NewNode { kind: NodeKind::Symbol, repo: repo.to_string(), path: Some(path.to_string()), name: Some(name.to_string()), container: None, start_line: Some(1), end_line: Some(2), content: Some("..".to_string()) }
    }

    #[test]
    fn walk_vault_parses_frontmatter_title_type_tags_and_status() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "gotchas/zoho.md", "---\ntitle: \"Zoho duplicate contact\"\ntype: gotcha\ntags: [zoho, crm]\nstatus: evergreen\n---\n\nBody text here.\n");

        let notes = walk_vault(dir.path(), &HeuristicClassifier).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Zoho duplicate contact");
        assert_eq!(notes[0].note_type, NoteType::Gotcha);
        assert_eq!(notes[0].tags, vec!["zoho", "crm"]);
        assert_eq!(notes[0].status.as_deref(), Some("evergreen"), "status must actually be parsed now, not just documented");
        assert_eq!(notes[0].body, "Body text here.");
    }

    #[test]
    fn walk_vault_infers_type_from_parent_folder_when_frontmatter_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "decisions/use-sqlite.md", "No frontmatter, just prose.\n");

        let notes = walk_vault(dir.path(), &HeuristicClassifier).unwrap();
        assert_eq!(notes[0].note_type, NoteType::Decision);
        assert_eq!(notes[0].title, "use-sqlite");
    }

    /// The actual point of this module: a note with NO organizational
    /// signal at all (no frontmatter, no folder convention) must still get
    /// a real classification from its content, not a silent Knowledge dump.
    #[test]
    fn walk_vault_classifies_freeform_gotcha_shaped_note_with_no_frontmatter_or_folder_signal() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "NOTES.md", "There's a known issue where the cache silently returns stale data — this is a workaround until we fix it properly.\n");

        let notes = walk_vault(dir.path(), &HeuristicClassifier).unwrap();
        assert_eq!(notes[0].note_type, NoteType::Gotcha, "gotcha-shaped language must be classified as a gotcha even with zero structural signal");
    }

    #[test]
    fn walk_vault_classifies_freeform_decision_shaped_note_with_no_frontmatter_or_folder_signal() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "README.md", "We decided to use SQLite instead of Postgres for the light tier, as a tradeoff favoring zero-infra deployment.\n");

        let notes = walk_vault(dir.path(), &HeuristicClassifier).unwrap();
        assert_eq!(notes[0].note_type, NoteType::Decision);
    }

    #[test]
    fn walk_vault_falls_back_to_first_h1_for_title_when_frontmatter_has_none() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "knowledge/misc.md", "---\ntype: knowledge\n---\n\n# The Real Title\n\nBody.\n");

        let notes = walk_vault(dir.path(), &HeuristicClassifier).unwrap();
        assert_eq!(notes[0].title, "The Real Title");
        assert_eq!(notes[0].note_type, NoteType::Knowledge);
    }

    #[test]
    fn walk_vault_only_reads_markdown_files_recursively() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "a/b/c/deep.md", "deep note\n");
        write_note(dir.path(), "notes.txt", "not markdown, must be ignored\n");

        let notes = walk_vault(dir.path(), &HeuristicClassifier).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].source_path.to_string_lossy().contains("deep.md"));
    }

    #[test]
    fn match_symbols_prefers_longer_more_specific_names_and_respects_min_len() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node("demo", "a.rs", "enrichZohoContact")).unwrap();
        upsert_node(&store, node("demo", "b.rs", "session")).unwrap();
        upsert_node(&store, node("demo", "c.rs", "id")).unwrap();

        let matches = match_symbols(&store, "demo", "Calls enrichZohoContact during session id lookup.", 4).unwrap();
        let lens: Vec<_> = matches.iter().map(|(_, len)| *len).collect();
        assert_eq!(matches.len(), 2, "the 2-char name below min_name_len must be excluded: {matches:?}");
        assert!(lens.contains(&"enrichZohoContact".len()));
        assert!(lens.contains(&"session".len()));
    }

    #[test]
    fn match_symbols_excludes_names_repeated_across_many_files_as_framework_noise() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        for path in ["a/route.js", "b/route.js", "c/route.js", "d/route.js"] {
            upsert_node(&store, node("demo", path, "POST")).unwrap();
        }
        upsert_node(&store, node("demo", "zoho.js", "enrichZohoContact")).unwrap();

        let matches = match_symbols(&store, "demo", "Sends a POST request that calls enrichZohoContact internally.", 4).unwrap();
        assert_eq!(matches.len(), 1, "POST must be excluded as framework noise: {matches:?}");
    }

    #[test]
    fn match_symbols_is_repo_scoped() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node("other-repo", "a.rs", "verify_token")).unwrap();

        let matches = match_symbols(&store, "demo", "Calls verify_token here.", 4).unwrap();
        assert!(matches.is_empty(), "a same-named symbol in a different repo must not match");
    }

    #[test]
    fn connect_many_reinforces_instead_of_duplicating_an_existing_edge() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let note_id = store.add_node(NewNode { kind: NodeKind::Note, repo: "demo".into(), path: None, name: Some("n".into()), container: None, start_line: None, end_line: None, content: Some("x".into()) }).unwrap();
        let target_id = upsert_node(&store, node("demo", "a.rs", "f")).unwrap();

        let first = connect_many(&store, "demo", note_id, &[target_id], EdgeRelation::Affects, true).unwrap();
        let second = connect_many(&store, "demo", note_id, &[target_id], EdgeRelation::Affects, true).unwrap();

        assert_eq!(first, ConnectSummary { added: 1, reinforced: 0 });
        assert_eq!(second, ConnectSummary { added: 0, reinforced: 1 }, "re-connecting the same target must reinforce it, not duplicate or silently no-op");

        let edges = store.edges_from("demo", note_id).unwrap();
        assert_eq!(edges.len(), 1, "still exactly one edge, not duplicated");
        assert_eq!(edges[0].weight, 1.5, "the second call's reinforcement must have bumped the weight");
    }

    #[test]
    fn ingest_vault_creates_typed_nodes_and_affects_edges_from_note_to_symbol() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, node("demo", "auth.rs", "verify_token")).unwrap();

        let notes = vec![VaultNote {
            title: "token bug".into(),
            note_type: NoteType::Gotcha,
            tags: vec![],
            status: None,
            body: "verify_token has an off-by-one bug.".into(),
            source_path: PathBuf::from("gotchas/token.md"),
        }];

        let matcher = WordBoundaryMatcher::default();
        let summary = ingest_vault(&store, "demo", &notes, &matcher, true).unwrap();

        assert_eq!(summary.notes_written, 1);
        assert_eq!(summary.edges_written, 1);
        let gotchas = store.nodes_by_kind("demo", NodeKind::Gotcha).unwrap();
        assert_eq!(gotchas.len(), 1);
        assert_eq!(gotchas[0].name.as_deref(), Some("token bug"));

        // Direction contract agentops-docgen depends on: note -> symbol.
        let symbol_id = store.nodes_by_kind("demo", NodeKind::Symbol).unwrap()[0].id;
        let edges = store.edges_from("demo", gotchas[0].id).unwrap();
        assert!(edges.iter().any(|e| e.dst_id == symbol_id && e.relation == EdgeRelation::Affects));
    }

    #[test]
    fn re_ingesting_an_unchanged_vault_updates_in_place_instead_of_duplicating() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let notes = vec![VaultNote { title: "n".into(), note_type: NoteType::Knowledge, tags: vec![], status: None, body: "body".into(), source_path: PathBuf::from("knowledge/n.md") }];
        let matcher = WordBoundaryMatcher::default();

        ingest_vault(&store, "demo", &notes, &matcher, true).unwrap();
        ingest_vault(&store, "demo", &notes, &matcher, true).unwrap();

        assert_eq!(store.nodes_by_kind("demo", NodeKind::Note).unwrap().len(), 1, "re-ingesting the same vault must not duplicate notes");
    }
}
