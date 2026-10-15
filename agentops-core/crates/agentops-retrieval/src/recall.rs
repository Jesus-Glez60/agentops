//! Cue-driven recall for hooks: given what the user just said, which
//! recorded gotchas, decisions, notes and doc sections should the agent
//! know about before it starts? Lexical and graph only. A hook runs in a
//! fresh process on every prompt, so this never loads the embedding model
//! (milestone 1 decision; semantic search stays in the planning step).
//!
//! Signals, fused by reciprocal rank:
//! - **Keywords.** The prompt is cut into keywords and each one is searched
//!   separately with `GraphStore::search_lexical`, per note kind. Separate
//!   queries rather than one, because the two adapters disagree on how a
//!   multi-word query combines: SQLite's FTS5 query ORs the terms, Postgres's
//!   `plainto_tsquery` ANDs them, so a whole prompt sent to Postgres would
//!   almost never match. A node matching more keywords ranks higher.
//! - **Named symbols.** Identifier-shaped words in the prompt
//!   (`snake_case`, `CamelCase`, `a::b`, backticked) are matched exactly
//!   against symbol names with `search_exact`, then followed one hop over
//!   `Affects` edges to the notes recorded against that symbol.
//!
//! Output is pointers, not content: kind, title, node id and why it
//! matched. The agent fetches the body with `fetch_content` if it needs it.

use std::collections::{HashMap, HashSet};

use agentops_graph::{EdgeRelation, GraphStore, NodeKind, NodeProminence};
use anyhow::Result;

/// The node kinds recall points at: recorded knowledge, never code.
pub const RECALL_KINDS: &[NodeKind] = &[NodeKind::Gotcha, NodeKind::Decision, NodeKind::Note, NodeKind::DocSection];

/// Keywords searched per prompt. Bounds the number of store queries a hook
/// makes (`MAX_KEYWORDS * RECALL_KINDS.len()`).
const MAX_KEYWORDS: usize = 8;
/// Candidates fetched per keyword and kind before fusion.
const PER_QUERY: usize = 10;

/// One recalled node, as a pointer the agent can follow.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallPointer {
    pub node_id: i64,
    pub kind: NodeKind,
    pub title: String,
    /// Why it matched: the keywords it matched, or the symbol it affects.
    pub reason: String,
}

impl RecallPointer {
    /// One line: `- [gotcha] Title (id 42) — matched "hook", "remote"`.
    pub fn render(&self) -> String {
        format!("- [{}] {} (id {}) — {}", self.kind.as_db_str(), self.title, self.node_id, self.reason)
    }
}

/// Up to `limit` pointers for `query` in `repo`, best first, skipping any
/// node id in `exclude` (already shown this session) and pinned nodes
/// (the pin block already shows those). Reduced-prominence nodes still
/// appear, ranked lower, since a curated-down gotcha is still knowledge
/// (ADR 0016).
pub fn recall_seed(store: &dyn GraphStore, repo: &str, query: &str, limit: usize, exclude: &HashSet<i64>) -> Result<Vec<RecallPointer>> {
    struct Candidate {
        score: f64,
        kind: NodeKind,
        title: String,
        prominence: NodeProminence,
        reasons: Vec<String>,
    }
    let mut candidates: HashMap<i64, Candidate> = HashMap::new();
    let mut add = |id: i64, kind: NodeKind, title: Option<&str>, prominence: NodeProminence, score: f64, reason: String| {
        let c = candidates.entry(id).or_insert_with(|| Candidate { score: 0.0, kind, title: title.unwrap_or("(untitled)").to_string(), prominence, reasons: Vec::new() });
        c.score += score;
        if !c.reasons.contains(&reason) {
            c.reasons.push(reason);
        }
    };

    for keyword in keywords(query) {
        for &kind in RECALL_KINDS {
            for (rank, (node, _)) in store.search_lexical(repo, &keyword, PER_QUERY, Some(kind))?.into_iter().enumerate() {
                add(node.id, node.kind, node.name.as_deref(), node.prominence, 1.0 / (rank as f64 + 1.0), format!("\"{keyword}\""));
            }
        }
    }

    // An exactly named symbol is a strong cue: its notes rank like a
    // top-ranked keyword hit for each symbol named.
    for identifier in identifiers(query) {
        for (symbol, score) in store.search_exact(repo, &identifier, 3, Some(NodeKind::Symbol))? {
            if score != 0.0 {
                continue; // substring match only, not the named symbol
            }
            for edge in store.edges_to(repo, symbol.id)? {
                if edge.relation != EdgeRelation::Affects {
                    continue;
                }
                let Some(note) = store.get_node(repo, edge.src_id)? else { continue };
                if RECALL_KINDS.contains(&note.kind) {
                    add(note.id, note.kind, note.name.as_deref(), note.prominence, 1.0, format!("affects `{identifier}`"));
                }
            }
        }
    }

    let mut ranked: Vec<(i64, Candidate)> = candidates.into_iter().filter(|(id, c)| !exclude.contains(id) && c.prominence != NodeProminence::Pinned).collect();
    for (_, c) in &mut ranked {
        if c.prominence == NodeProminence::Reduced {
            c.score *= 0.5;
        }
    }
    // Score first; id as a stable tiebreak so the same prompt always
    // yields the same seed.
    ranked.sort_by(|a, b| b.1.score.total_cmp(&a.1.score).then(a.0.cmp(&b.0)));
    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|(id, c)| {
            let reason = if c.reasons.iter().all(|r| r.starts_with('"')) { format!("matched {}", c.reasons.join(", ")) } else { c.reasons.join("; ") };
            RecallPointer { node_id: id, kind: c.kind, title: c.title, reason }
        })
        .collect())
}

/// Common English and instruction words that would match half the graph.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "onto", "are", "was", "were", "been", "being", "have", "has", "had", "not", "but", "can", "could", "should", "would", "will", "shall", "may", "might", "must", "you", "your", "our", "its", "their", "them", "they", "what", "when", "where", "which", "while", "who", "why", "how", "all", "any", "each", "some", "more", "most", "other", "such", "only", "own", "same", "than", "too", "very", "just", "also", "then", "there", "here", "about", "above", "after", "again", "before", "below", "between", "both", "during", "further", "once", "out", "over", "under", "until", "make", "made", "want", "need", "like", "use", "using", "add", "get", "let", "please", "lets", "now", "new", "way", "does", "did", "doing", "done", "one", "two", "it's", "i'm", "don't", "can't", "sure", "plan", "task", "fix", "change", "update", "better", "work", "thing", "things",
];

/// Distinct keywords from `query`, in order of first appearance: words of
/// three or more characters that aren't stopwords, punctuation stripped
/// from the ends. Identifier pieces (`hook_capture` → `hook_capture`) stay
/// whole, since FTS tokenizes them itself.
fn keywords(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    query
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '`' | '?' | '!'))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_').to_lowercase())
        .filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(&w.as_str()) && !w.chars().all(|c| c.is_ascii_digit()))
        .filter(|w| seen.insert(w.clone()))
        .take(MAX_KEYWORDS)
        .collect()
}

/// Identifier-shaped words in `query`: backticked spans, and bare words
/// containing `_` or `::`, or mixing lower and upper case. Only the last
/// `::` segment is kept, since symbol names are stored unqualified.
fn identifiers(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let backticked = query.split('`').skip(1).step_by(2).map(str::to_string);
    let bare = query.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != ':').to_string()).filter(|w| {
        let has_lower = w.chars().any(|c| c.is_lowercase());
        let has_upper = w.chars().any(|c| c.is_uppercase());
        w.contains('_') || w.contains("::") || (has_lower && has_upper && w.chars().next().is_some_and(|c| c.is_alphabetic()) && w.chars().skip(1).any(|c| c.is_uppercase()))
    });
    for raw in backticked.chain(bare) {
        let name = raw.rsplit("::").next().unwrap_or(&raw).trim_end_matches("()").trim().to_string();
        if name.chars().count() >= 3 && !name.contains(char::is_whitespace) && seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{upsert_node, NewNode, SqliteGraphStore};

    fn note(store: &SqliteGraphStore, kind: NodeKind, name: &str, content: &str) -> i64 {
        upsert_node(store, NewNode { kind, repo: "r".into(), path: Some(format!("vault:{name}")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap()
    }

    #[test]
    fn keywords_drop_stopwords_short_words_and_duplicates() {
        assert_eq!(keywords("Add a --json flag to the agentops status command, json please"), vec!["json", "flag", "agentops", "status", "command"]);
    }

    #[test]
    fn identifiers_find_snake_case_paths_camel_case_and_backticks() {
        assert_eq!(identifiers("why does hook_capture call agentops_mcp::open_store and `RecallPointer` but not Hello"), vec!["RecallPointer", "hook_capture", "open_store"]);
    }

    #[test]
    fn a_note_matching_more_keywords_ranks_first_and_says_why() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let both = note(&store, NodeKind::Gotcha, "Remote hooks ignore the server", "hooks open the local store even when remote");
        let one = note(&store, NodeKind::Decision, "Hooks are opt-in", "hooks are installed only with a flag");
        note(&store, NodeKind::Symbol, "remote_hooks", "fn remote_hooks() {}");
        let seed = recall_seed(&store, "r", "make remote hooks reliable", 8, &HashSet::new()).unwrap();
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![both, one], "symbols never appear; more matched keywords rank higher: {seed:?}");
        assert!(seed[0].render().contains("matched \"remote\", \"hooks\""), "{}", seed[0].render());
    }

    #[test]
    fn excluded_and_pinned_nodes_are_skipped_and_the_limit_holds() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = note(&store, NodeKind::Gotcha, "ingest drops tags", "ingest");
        let b = note(&store, NodeKind::Gotcha, "ingest is slow", "ingest");
        let c = note(&store, NodeKind::Gotcha, "ingest pinned rule", "ingest");
        store.set_curation("r", c, NodeProminence::Pinned, None).unwrap();
        let seed = recall_seed(&store, "r", "ingest", 8, &HashSet::from([a])).unwrap();
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![b]);
        assert!(recall_seed(&store, "r", "ingest", 0, &HashSet::new()).unwrap().is_empty());
    }

    #[test]
    fn a_named_symbol_pulls_in_the_notes_that_affect_it() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol = note(&store, NodeKind::Symbol, "ingest_vault", "fn ingest_vault() {}");
        let gotcha = note(&store, NodeKind::Gotcha, "Tags never reach the graph", "only the body is persisted");
        store.add_edge("r", gotcha, symbol, EdgeRelation::Affects).unwrap();
        let seed = recall_seed(&store, "r", "look at ingest_vault", 8, &HashSet::new()).unwrap();
        assert_eq!(seed.len(), 1, "{seed:?}");
        assert_eq!(seed[0].node_id, gotcha);
        assert!(seed[0].reason.contains("affects `ingest_vault`"), "{seed:?}");
    }

    #[test]
    fn a_prompt_with_no_usable_keywords_recalls_nothing() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        note(&store, NodeKind::Gotcha, "anything", "the and for");
        assert!(recall_seed(&store, "r", "do it now", 8, &HashSet::new()).unwrap().is_empty());
    }
}
