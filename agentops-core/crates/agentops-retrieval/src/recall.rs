//! Cue-driven recall for hooks: given what the user just said, which
//! recorded gotchas, decisions and notes should the agent
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
//!   almost never match. A node matching more keywords ranks higher. With
//!   three or more keywords a node must match at least two, and the repo's
//!   own name never counts as a keyword.
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

/// The node kinds recall points at: memories, never code. Generated doc
/// sections are left out too: digests such as "Known Gotchas" contain
/// every note's words, so they matched almost any prompt and filled three
/// of eight seed slots on every one (F53). They stay in search, the
/// gist-then-detail tier and the repo briefing, which orient rather than
/// recall.
pub const RECALL_KINDS: &[NodeKind] = &[NodeKind::Gotcha, NodeKind::Decision, NodeKind::Note];

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

    // The repo's own name appears in half its notes, so it never counts as
    // a keyword (hosted repos are named `<owner>--<repo>--<tenant>`).
    let repo_lower = repo.to_lowercase();
    let keywords: Vec<String> = keywords(query).into_iter().filter(|k| !repo_lower.contains(k.as_str())).collect();
    // With several keywords, one weak word in common isn't a cue: a note
    // must match at least two. An empty seed beats a noisy one, which only
    // teaches the agent to skip seeds.
    let min_keyword_matches = if keywords.len() >= 3 { 2 } else { 1 };
    for keyword in &keywords {
        for &kind in RECALL_KINDS {
            for (rank, (node, _)) in store.search_lexical(repo, keyword, PER_QUERY, Some(kind))?.into_iter().enumerate() {
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

    let enough = |c: &Candidate| c.reasons.iter().any(|r| r.starts_with("affects")) || c.reasons.len() >= min_keyword_matches;
    let shown = |id: &i64, prominence: NodeProminence| !exclude.contains(id) && prominence != NodeProminence::Pinned && prominence.recallable();
    // How many distinct cues a node matched comes first: rank fusion runs
    // per kind, so every kind's top hit scores alike, and a gotcha matching
    // three keywords must beat a decision matching two (seen live on the
    // server graph). A named symbol's note counts as one cue more.
    let mut ranked: Vec<Ranked> = candidates
        .into_iter()
        .filter(|(id, c)| shown(id, c.prominence) && enough(c))
        .map(|(id, c)| {
            let cues = c.reasons.len() + usize::from(c.reasons.iter().any(|r| r.starts_with("affects")));
            let score = if c.prominence == NodeProminence::Reduced { c.score * 0.5 } else { c.score };
            Ranked { id, cues, score, hop: false, kind: c.kind, title: c.title, reasons: c.reasons }
        })
        .collect();
    sort_ranked(&mut ranked);
    hop(store, repo, &mut ranked, &shown)?;
    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|r| {
            let reason = if r.reasons.iter().all(|x| x.starts_with('"')) { format!("matched {}", r.reasons.join(", ")) } else { r.reasons.join("; ") };
            RecallPointer { node_id: r.id, kind: r.kind, title: r.title, reason }
        })
        .collect())
}

/// What a neighbour reached over `SimilarTo` keeps of its source's score,
/// on top of their cosine: a neighbour of a strong match can beat a weaker
/// direct hit, but never its own source.
pub const HOP_FACTOR: f64 = 0.8;
/// Neighbours one source brings into the seed (or lifts within it), best
/// cosine first. Without a cap a strongly matched hub memory, with up to
/// `SIMILAR_MAX_DEGREE` edges, filled 5-7 of 8 seed slots with its own
/// neighbours and recall fell from 13/20 to 9/20 (milestone 3a eval).
pub const HOP_MAX_PER_SOURCE: usize = 2;

/// A recall candidate with its strength: cues first, then score.
struct Ranked {
    id: i64,
    cues: usize,
    score: f64,
    /// Reached only through a `SimilarTo` hop, not on its own cues.
    hop: bool,
    kind: NodeKind,
    title: String,
    reasons: Vec<String>,
}

/// Strongest first: cues, then a direct match before a hop neighbour, then
/// score, then id as a stable tiebreak.
fn sort_ranked(ranked: &mut [Ranked]) {
    ranked.sort_by(|a, b| b.cues.cmp(&a.cues).then(a.hop.cmp(&b.hop)).then(b.score.total_cmp(&a.score)).then(a.id.cmp(&b.id)));
}

/// One `SimilarTo` hop from each memory that made it on its own cues
/// (ADR 0051), so a memory that names no symbol and shares no keyword can
/// still come up beside the one it resembles. A neighbour counts as one
/// cue, ranks after the direct matches with as many, and scores `source
/// score × cosine × HOP_FACTOR` (halved again if it's reduced), so among
/// neighbours the closest one of the strongest match comes first. Inheriting the source's cue count, in
/// full or one step down, lost 2 of 20 eval cases against this: the
/// neighbours of a strong match pushed out the direct hits that were the
/// answer (milestone 3a item 3, reverted with the author's approval). Edges keep no score, so the
/// cosine comes from the two stored vectors; a neighbour without one isn't
/// reached. Each source reaches at most `HOP_MAX_PER_SOURCE` neighbours,
/// its closest. A node reached directly and by a hop, or by two hops, keeps
/// the stronger of them and every reason. Sources are walked strongest
/// first, so the result never depends on map order.
fn hop(store: &dyn GraphStore, repo: &str, ranked: &mut Vec<Ranked>, shown: &dyn Fn(&i64, NodeProminence) -> bool) -> Result<()> {
    let mut position: HashMap<i64, usize> = ranked.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
    let sources: Vec<(i64, f64, String)> = ranked.iter().map(|r| (r.id, r.score, r.title.clone())).collect();
    for (source_id, score, title) in sources {
        let targets: Vec<i64> = store.edges_from(repo, source_id)?.into_iter().filter(|e| e.relation == EdgeRelation::SimilarTo).map(|e| e.dst_id).collect();
        if targets.is_empty() {
            continue;
        }
        let vectors = store.get_embeddings(repo, &[&[source_id][..], &targets].concat())?;
        let Some(source_vector) = vectors.get(&source_id) else { continue };
        let mut closest: Vec<(f64, i64)> = targets.iter().filter(|&&t| t != source_id).filter_map(|t| vectors.get(t).map(|v| (f64::from(agentops_graph::similarity::cosine(source_vector, v)), *t))).collect();
        closest.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut taken = 0;
        for (cosine, target) in closest {
            if taken == HOP_MAX_PER_SOURCE {
                break;
            }
            let reason = format!("similar to \"{title}\"");
            let hop_cues = 1;
            if let Some(&i) = position.get(&target) {
                taken += 1;
                let r = &mut ranked[i];
                if (hop_cues, score * cosine * HOP_FACTOR) > (r.cues, r.score) {
                    (r.cues, r.score) = (hop_cues, score * cosine * HOP_FACTOR);
                }
                if !r.reasons.contains(&reason) {
                    r.reasons.push(reason);
                }
                continue;
            }
            let Some(neighbour) = store.get_node(repo, target)? else { continue };
            if !RECALL_KINDS.contains(&neighbour.kind) || !shown(&neighbour.id, neighbour.prominence) {
                continue;
            }
            let mut hop_score = score * cosine * HOP_FACTOR;
            if neighbour.prominence == NodeProminence::Reduced {
                hop_score *= 0.5;
            }
            taken += 1;
            position.insert(target, ranked.len());
            ranked.push(Ranked { id: target, cues: hop_cues, score: hop_score, hop: true, kind: neighbour.kind, title: neighbour.name.unwrap_or_else(|| "(untitled)".to_string()), reasons: vec![reason] });
        }
    }
    sort_ranked(ranked);
    Ok(())
}

/// Common English and instruction words that would match half the graph.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "onto", "are", "was", "were", "been", "being", "have", "has", "had", "not", "but", "can", "could", "should", "would", "will", "shall", "may", "might", "must", "you", "your", "our", "its", "their", "them", "they", "what", "when", "where", "which", "while", "who", "why", "how", "all", "any", "each", "some", "more", "most", "other", "such", "only", "own", "same", "than", "too", "very", "just", "also", "then", "there", "here", "about", "above", "after", "again", "before", "below", "between", "both", "during", "further", "once", "out", "over", "under", "until", "make", "made", "want", "need", "like", "use", "using", "add", "get", "let", "please", "lets", "now", "new", "way", "does", "did", "doing", "done", "one", "two", "it's", "i'm", "don't", "can't", "sure", "plan", "task", "fix", "change", "update", "better", "work", "works", "working", "thing", "things", "fail", "fails", "failed", "failing", "check", "checks", "ignore", "ignores", "broken", "break", "breaks", "issue", "problem", "wrong", "doesn't", "isn't", "look", "see", "try",
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
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![both], "with 3 keywords, one shared word isn't enough; symbols never appear: {seed:?}");
        let seed = recall_seed(&store, "r", "remote hooks", 8, &HashSet::new()).unwrap();
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![both, one], "more matched keywords rank higher: {seed:?}");
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
    fn a_similar_to_hop_reaches_a_memory_with_no_keyword_and_no_symbol_and_says_why() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let matched = note(&store, NodeKind::Gotcha, "Webhook signature check fails", "webhook signature 401");
        let associated = note(&store, NodeKind::Note, "Rotate the app secret after a deploy", "rotate secret deploy");
        let unrelated = note(&store, NodeKind::Note, "Unrelated memory", "nothing in common");
        let unsettled = note(&store, NodeKind::Decision, "Unsettled association", "pending");
        store.set_curation("r", unsettled, NodeProminence::NeedsReview, Some("unclassified on import")).unwrap();
        for (a, b) in [(matched, associated), (matched, unsettled)] {
            store.add_edge("r", a, b, EdgeRelation::SimilarTo).unwrap();
            store.add_edge("r", b, a, EdgeRelation::SimilarTo).unwrap();
        }
        for (id, tilt) in [(matched, 0.0), (associated, 0.3), (unsettled, 0.3)] {
            store.set_embedding("r", id, &vector(tilt)).unwrap();
        }

        let seed = recall_seed(&store, "r", "webhook signature", 8, &HashSet::new()).unwrap();
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![matched, associated], "{seed:?}");
        assert_eq!(seed[1].reason, "similar to \"Webhook signature check fails\"");
        assert!(seed.iter().all(|p| p.node_id != unrelated && p.node_id != unsettled));
        // Excluded or already-shown neighbours stay out too.
        let again = recall_seed(&store, "r", "webhook signature", 8, &HashSet::from([associated])).unwrap();
        assert_eq!(again.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![matched]);
    }

    /// A full-size unit-ish vector leaning `tilt` off the first axis.
    fn vector(tilt: f32) -> Vec<f32> {
        let mut v = vec![0.0; agentops_embeddings::EMBEDDING_DIM];
        (v[0], v[1]) = (1.0, tilt);
        v
    }

    fn link(store: &SqliteGraphStore, a: i64, b: i64) {
        store.add_edge("r", a, b, EdgeRelation::SimilarTo).unwrap();
        store.add_edge("r", b, a, EdgeRelation::SimilarTo).unwrap();
    }

    #[test]
    fn a_neighbour_counts_as_one_cue_and_ranks_after_the_direct_hits() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let source = note(&store, NodeKind::Gotcha, "Webhook signature deploy rollback", "webhook signature deploy rollback");
        let equal = note(&store, NodeKind::Decision, "Webhook signature deploy order", "webhook signature deploy");
        let weaker = note(&store, NodeKind::Note, "Webhook signature rotation", "webhook signature");
        let neighbour = note(&store, NodeKind::Note, "Rotate the app secret", "nothing shared");
        link(&store, source, neighbour);
        store.set_embedding("r", source, &vector(0.0)).unwrap();
        store.set_embedding("r", neighbour, &vector(0.1)).unwrap();
        let seed = recall_seed(&store, "r", "webhook signature deploy rollback", 8, &HashSet::new()).unwrap();
        // Every direct hit with two or more cues, then the one-cue neighbour.
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![source, equal, weaker, neighbour], "{seed:?}");
        assert_eq!(seed[3].reason, "similar to \"Webhook signature deploy rollback\"");

    }

    #[test]
    fn a_node_reached_directly_and_by_a_hop_keeps_the_stronger_and_both_reasons() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let strong = note(&store, NodeKind::Gotcha, "Webhook signature deploy failure", "webhook signature deploy");
        let weak = note(&store, NodeKind::Decision, "Webhook signature rotation", "webhook signature");
        link(&store, strong, weak);
        store.set_embedding("r", strong, &vector(0.0)).unwrap();
        store.set_embedding("r", weak, &vector(0.1)).unwrap();
        let seed = recall_seed(&store, "r", "webhook signature deploy", 8, &HashSet::new()).unwrap();
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![strong, weak]);
        assert_eq!(seed[1].reason, "\"webhook\"; \"signature\"; similar to \"Webhook signature deploy failure\"", "{seed:?}");
        assert!(seed[0].reason.starts_with("\"webhook\"; \"signature\"; \"deploy\""), "the reverse edge only adds a reason, never strength: {seed:?}");
    }

    #[test]
    fn one_source_brings_at_most_its_two_closest_neighbours() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let hub = note(&store, NodeKind::Gotcha, "Webhook signature deploy failure", "webhook signature deploy");
        store.set_embedding("r", hub, &vector(0.0)).unwrap();
        let neighbours: Vec<i64> = (0..6)
            .map(|n| {
                let id = note(&store, NodeKind::Note, &format!("Neighbour {n}"), "nothing shared");
                link(&store, hub, id);
                store.set_embedding("r", id, &vector(0.1 * (n as f32 + 1.0))).unwrap();
                id
            })
            .collect();
        let seed = recall_seed(&store, "r", "webhook signature deploy", 8, &HashSet::new()).unwrap();
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![hub, neighbours[0], neighbours[1]], "the two closest only: {seed:?}");
    }

    #[test]
    fn the_same_prompt_on_the_same_graph_always_gives_the_same_seed() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let sources: Vec<i64> = (0..6).map(|n| note(&store, NodeKind::Gotcha, &format!("webhook signature {n}"), "webhook signature")).collect();
        let shared = note(&store, NodeKind::Note, "Shared neighbour", "nothing");
        for (i, &s) in sources.iter().enumerate() {
            link(&store, s, shared);
            store.set_embedding("r", s, &vector(i as f32 * 0.1)).unwrap();
        }
        store.set_embedding("r", shared, &vector(0.25)).unwrap();
        let first = recall_seed(&store, "r", "webhook signature", 8, &HashSet::new()).unwrap();
        for _ in 0..20 {
            assert_eq!(recall_seed(&store, "r", "webhook signature", 8, &HashSet::new()).unwrap(), first);
        }
    }

    #[test]
    fn a_needs_review_node_never_reaches_the_seed_by_keyword_or_by_symbol() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let kept = note(&store, NodeKind::Gotcha, "ingest drops tags", "ingest_vault");
        let unsettled = note(&store, NodeKind::Note, "ingest maybe slow", "ingest_vault");
        store.set_curation("r", unsettled, NodeProminence::NeedsReview, Some("unclassified on import")).unwrap();
        let symbol = note(&store, NodeKind::Symbol, "ingest_vault", "fn ingest_vault() {}");
        store.add_edge("r", unsettled, symbol, EdgeRelation::Affects).unwrap();
        let by_keyword = recall_seed(&store, "r", "ingest", 8, &HashSet::new()).unwrap();
        let by_symbol = recall_seed(&store, "r", "why is ingest_vault slow", 8, &HashSet::new()).unwrap();
        assert_eq!(by_keyword.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![kept]);
        assert!(by_symbol.iter().all(|p| p.node_id != unsettled), "{by_symbol:?}");
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
    fn the_repos_own_name_is_never_a_keyword() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "owner--agentops--t1".into(), path: Some("vault:a".into()), name: Some("agentops naming".into()), container: None, start_line: None, end_line: None, content: Some("agentops agentops".into()) }).unwrap();
        assert!(recall_seed(&store, "owner--agentops--t1", "agentops status json", 8, &HashSet::new()).unwrap().is_empty());
    }

    #[test]
    fn a_doc_section_never_reaches_a_seed_by_keyword_or_by_symbol() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let digest = note(&store, NodeKind::DocSection, "Known Gotchas", "webhook signature deadlock pool postgres everything");
        let specific = note(&store, NodeKind::Gotcha, "Webhook signature mismatch", "every webhook gets 401");
        let symbol = note(&store, NodeKind::Symbol, "verify_signature", "fn verify_signature() {}");
        store.add_edge("r", digest, symbol, EdgeRelation::Affects).unwrap();
        let seed = recall_seed(&store, "r", "webhook signature", 8, &HashSet::new()).unwrap();
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![specific], "{seed:?}");
        assert!(recall_seed(&store, "r", "why does verify_signature fail", 8, &HashSet::new()).unwrap().iter().all(|p| p.kind != NodeKind::DocSection));
    }

    #[test]
    fn a_note_matching_more_keywords_beats_the_top_hit_of_another_kind() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let decision = note(&store, NodeKind::Decision, "Webhook auto-kickoff", "github webhook");
        let gotcha = note(&store, NodeKind::Gotcha, "Webhook signature 401", "github webhook signature invalid");
        let seed = recall_seed(&store, "r", "github webhook signature", 8, &HashSet::new()).unwrap();
        assert_eq!(seed.iter().map(|p| p.node_id).collect::<Vec<_>>(), vec![gotcha, decision], "{seed:?}");
    }

    #[test]
    fn a_prompt_with_no_usable_keywords_recalls_nothing() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        note(&store, NodeKind::Gotcha, "anything", "the and for");
        assert!(recall_seed(&store, "r", "do it now", 8, &HashSet::new()).unwrap().is_empty());
    }
}
