//! Multi-signal retrieval (Phase 4, 1.0 roadmap) — fuses dense (embedding),
//! lexical (BM25/FTS), and exact-name-match candidate lists into one ranked
//! result, so a literal function-name query reliably surfaces it even when
//! embedding similarity alone wouldn't rank it highly, and a keyword-heavy
//! query gets real IDF-weighted relevance instead of relying on semantic
//! similarity alone.
//!
//! Redesigned from scratch against this rebuild's actual architecture, not
//! ported from the vault's original spec: that spec assumed a separate
//! heavy-tier/Qdrant dense index and a new tantivy-backed lexical crate,
//! written before this session's own earlier passes moved dense search into
//! `agentops-core` (`agentops-embeddings` + `GraphStore::search_similar`,
//! sqlite-vec/pgvector) and there was no reason left to introduce a second
//! external index for lexical search either — see `search_lexical`'s own
//! doc comment on `GraphStore` for why FTS5/`tsvector` (database-native,
//! auto-kept-in-sync) won over a tantivy port. No cross-encoder rerank step
//! exists in `agentops-core` today (that stayed heavy-tier/Qdrant-only, in
//! `agentops-heavy-embeddings`) — this crate's fusion *is* the ranking
//! signal for core's hybrid search, not a pre-rerank candidate pool.

use agentops_embeddings::Embedder;
use agentops_graph::{GraphStore, Node, NodeKind};
use anyhow::Result;

/// One retrieval signal, returning candidate node ids **already ordered
/// best-first** — `search_hybrid`'s fusion only ever looks at rank
/// position, never at a source's raw score, since dense (cosine distance,
/// lower better), lexical (SQLite `bm25`, lower better; Postgres
/// `ts_rank`, higher better), and exact (a fixed 0.0/1.0) all use
/// different, non-comparable scales.
pub trait CandidateSource {
    fn name(&self) -> &'static str;
    fn search(&self, store: &dyn GraphStore, repo: &str, query: &str, kind: Option<NodeKind>, top_k: usize) -> Result<Vec<i64>>;
}

/// Embeds `query` and delegates to `GraphStore::search_similar`.
pub struct DenseCandidateSource<'a> {
    pub embedder: &'a dyn Embedder,
}

impl CandidateSource for DenseCandidateSource<'_> {
    fn name(&self) -> &'static str {
        "dense"
    }

    fn search(&self, store: &dyn GraphStore, repo: &str, query: &str, kind: Option<NodeKind>, top_k: usize) -> Result<Vec<i64>> {
        let embedding = self.embedder.embed(query)?;
        Ok(store.search_similar(repo, &embedding, top_k, kind)?.into_iter().map(|(n, _)| n.id).collect())
    }
}

pub struct LexicalCandidateSource;

impl CandidateSource for LexicalCandidateSource {
    fn name(&self) -> &'static str {
        "lexical"
    }

    fn search(&self, store: &dyn GraphStore, repo: &str, query: &str, kind: Option<NodeKind>, top_k: usize) -> Result<Vec<i64>> {
        Ok(store.search_lexical(repo, query, top_k, kind)?.into_iter().map(|(n, _)| n.id).collect())
    }
}

pub struct ExactCandidateSource;

impl CandidateSource for ExactCandidateSource {
    fn name(&self) -> &'static str {
        "exact"
    }

    fn search(&self, store: &dyn GraphStore, repo: &str, query: &str, kind: Option<NodeKind>, top_k: usize) -> Result<Vec<i64>> {
        Ok(store.search_exact(repo, query, top_k, kind)?.into_iter().map(|(n, _)| n.id).collect())
    }
}

/// One fused hit — `fused_score` is only meaningful relative to other hits
/// in the same `search_hybrid` call (Reciprocal Rank Fusion output, not a
/// probability/distance). `*_rank` fields (0-based, `None` if that source
/// didn't return this node at all) are exposed so a caller/UI can show
/// *why* something ranked where it did — the vault's original
/// `signal_scores: {dense, lexical, exact}` API-surface note, adapted to
/// ranks rather than incomparable raw scores.
#[derive(Debug, Clone)]
pub struct HybridHit {
    pub node: Node,
    pub fused_score: f32,
    pub dense_rank: Option<usize>,
    pub lexical_rank: Option<usize>,
    pub exact_rank: Option<usize>,
}

/// Standard RRF constant — dampens the influence of any single source's
/// exact rank ordering among its own top results, so one source dominating
/// its own list doesn't overwhelm the fused ranking.
const RRF_K: f32 = 60.0;

/// Fuses dense + lexical + exact candidate lists via Reciprocal Rank
/// Fusion: `score(node) = sum over sources containing it of 1 / (RRF_K +
/// rank + 1)`. Each source is over-fetched at `top_k * 3` internally so the
/// fusion has enough candidates to work with even when the final `top_k` is
/// small, then the fused, sorted result is truncated to `top_k`.
pub fn search_hybrid(store: &dyn GraphStore, embedder: &dyn Embedder, repo: &str, query: &str, top_k: usize, kind: Option<NodeKind>) -> Result<Vec<HybridHit>> {
    let fetch_k = (top_k * 3).max(10);
    let dense = DenseCandidateSource { embedder }.search(store, repo, query, kind, fetch_k)?;
    let lexical = LexicalCandidateSource.search(store, repo, query, kind, fetch_k)?;
    let exact = ExactCandidateSource.search(store, repo, query, kind, fetch_k)?;

    let rank_of = |ids: &[i64], id: i64| -> Option<usize> { ids.iter().position(|&x| x == id) };
    let rrf = |rank: Option<usize>| -> f32 { rank.map(|r| 1.0 / (RRF_K + r as f32 + 1.0)).unwrap_or(0.0) };

    let mut all_ids: Vec<i64> = dense.iter().chain(lexical.iter()).chain(exact.iter()).copied().collect();
    all_ids.sort_unstable();
    all_ids.dedup();

    let mut hits: Vec<HybridHit> = Vec::with_capacity(all_ids.len());
    for id in all_ids {
        let Some(node) = store.get_node(repo, id)? else { continue };
        let dense_rank = rank_of(&dense, id);
        let lexical_rank = rank_of(&lexical, id);
        let exact_rank = rank_of(&exact, id);
        let fused_score = rrf(dense_rank) + rrf(lexical_rank) + rrf(exact_rank);
        hits.push(HybridHit { node, fused_score, dense_rank, lexical_rank, exact_rank });
    }

    // Sort by a damped copy of fused_score, not the field itself -- a
    // Reduced-prominence hit ranks lower (this is the MCP semantic_search
    // tool's actual live ranking, the surface an agent sees mid-session)
    // but `HybridHit.fused_score` stays the real RRF value callers display.
    hits.sort_by(|a, b| {
        let rank_a = a.fused_score * agentops_graph::prominence_rank_multiplier(a.node.prominence) as f32;
        let rank_b = b.fused_score * agentops_graph::prominence_rank_multiplier(b.node.prominence) as f32;
        rank_b.partial_cmp(&rank_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(top_k);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_embeddings::LocalEmbedder;
    use agentops_graph::{upsert_node, NewNode, SqliteGraphStore};

    fn symbol(store: &dyn GraphStore, repo: &str, path: &str, name: &str, content: &str) -> i64 {
        upsert_node(store, NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(path.into()), name: Some(name.into()), container: None, start_line: Some(1), end_line: Some(2), content: Some(content.into()) })
            .unwrap()
    }

    /// The literal use case Module 3's design was written for: an exact
    /// function-name query must surface it even though a raw dense-only
    /// search over unrelated-looking source text might not rank it highly
    /// (here, guaranteed by never embedding it at all).
    #[test]
    fn a_literal_symbol_name_query_surfaces_it_via_the_exact_signal_alone() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let target = symbol(&store, "demo", "auth.rs", "verify_token_signature", "fn verify_token_signature() { /* impl */ }");
        symbol(&store, "demo", "other.rs", "unrelated_helper", "fn unrelated_helper() {}");
        // Deliberately no `set_embedding` call for either node — the dense
        // signal contributes nothing here, proving exact-match alone can
        // carry the ranking.

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token_signature", 5, None).unwrap();
        assert!(!hits.is_empty(), "{hits:?}");
        assert_eq!(hits[0].node.id, target, "the exact name match must rank first: {hits:?}");
        assert!(hits[0].exact_rank.is_some());
        assert!(hits[0].dense_rank.is_none(), "no embedding was ever set for this node");
    }

    #[test]
    fn a_node_matched_by_multiple_signals_outranks_one_matched_by_only_one() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let multi = symbol(&store, "demo", "auth.rs", "check_expiry", "fn check_expiry() { validate token expiry logic here }");
        let lexical_only = symbol(&store, "demo", "other.rs", "something_else", "this mentions expiry logic in a comment only, unrelated name");
        store.set_embedding("demo", multi, &LocalEmbedder.embed("fn check_expiry() { validate token expiry logic here }").unwrap()).unwrap();
        store.set_embedding("demo", lexical_only, &LocalEmbedder.embed("this mentions expiry logic in a comment only, unrelated name").unwrap()).unwrap();

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "expiry logic", 5, None).unwrap();
        let multi_pos = hits.iter().position(|h| h.node.id == multi).expect("must appear");
        let lexical_only_pos = hits.iter().position(|h| h.node.id == lexical_only).expect("must appear");
        assert!(multi_pos < lexical_only_pos, "a node matched by more signals must fuse to a higher rank: {hits:?}");
    }

    #[test]
    fn search_hybrid_is_repo_scoped() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        symbol(&store, "other-repo", "a.rs", "verify_token", "fn verify_token() {}");

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, None).unwrap();
        assert!(hits.is_empty(), "a different repo's node must never appear: {hits:?}");
    }

    #[test]
    fn search_hybrid_respects_kind_scoping() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        symbol(&store, "demo", "a.rs", "verify_token", "fn verify_token() {}");
        upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("verify_token".into()), container: None, start_line: None, end_line: None, content: Some("a gotcha about verify_token".into()) }).unwrap();

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, Some(NodeKind::Symbol)).unwrap();
        assert!(hits.iter().all(|h| h.node.kind == NodeKind::Symbol), "{hits:?}");
    }

    #[test]
    fn a_reduced_prominence_hit_ranks_below_an_equally_matched_full_one_but_keeps_its_real_fused_score() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let full = symbol(&store, "demo", "a.rs", "verify_token_full", "fn verify_token_full() {}");
        let reduced = symbol(&store, "demo", "b.rs", "verify_token_reduced", "fn verify_token_reduced() {}");

        // Same node, before/after curation -- the one true test that
        // curation never alters the displayed fused_score, independent of
        // any incidental per-node rank differences elsewhere in the fusion.
        let score_before = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, None).unwrap().into_iter().find(|h| h.node.id == reduced).unwrap().fused_score;
        store.set_curation("demo", reduced, agentops_graph::NodeProminence::Reduced, Some("niche")).unwrap();
        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, None).unwrap();
        let reduced_hit = hits.iter().find(|h| h.node.id == reduced).expect("must still appear -- curation never hides a hit");
        assert_eq!(reduced_hit.fused_score, score_before, "the displayed fused_score itself must stay real/undamped, not silently altered by curation");

        let full_pos = hits.iter().position(|h| h.node.id == full).unwrap();
        let reduced_pos = hits.iter().position(|h| h.node.id == reduced).unwrap();
        assert!(full_pos < reduced_pos, "the Reduced-prominence hit must rank lower once curated: {hits:?}");
    }
}
