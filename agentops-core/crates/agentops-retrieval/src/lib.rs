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

use std::collections::HashMap;

use agentops_embeddings::Embedder;
use agentops_graph::{age_days, bounded_neighborhood, effective_edge_weight, effective_weight, EdgeRelation, GraphStore, Node, NodeKind, NeighborhoodQuery, TraversalDirection};
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

/// A trained per-repo embedding projection (Initiative 5, CLS-inspired
/// retrieval plan) -- deliberately just this one-method trait, not a
/// direct dependency on `agentops-embeddings-train`'s `ProjectionHead`.
/// That crate exists specifically to keep `candle-core`/`candle-nn` out of
/// the default inference path every caller of `search_hybrid` pulls in
/// (`agentops-mcp`, `agentops-api`, `agentops-cli`, `agentops-llm` all
/// depend on this crate); a caller that *does* depend on the training
/// crate (today, only `agentops-mcp`'s consolidation call sites) supplies
/// an implementation, this crate never constructs or loads one itself.
pub trait EmbeddingProjector {
    fn project(&self, embedding: &[f32]) -> Vec<f32>;
}

/// Embeds `query` and delegates to `GraphStore::search_similar`. If
/// `projector` is set, re-ranks the raw-similarity candidates by projected
/// cosine similarity instead (Initiative 5's query-time integration) --
/// `search_similar` itself still does the actual over-fetch/KNN work on
/// raw embeddings; the projection only re-orders what it already returned,
/// per the plan's "re-rank step over already-over-fetched candidates"
/// design (avoids needing a second, projected vector index).
pub struct DenseCandidateSource<'a> {
    pub embedder: &'a dyn Embedder,
    pub projector: Option<&'a dyn EmbeddingProjector>,
}

impl CandidateSource for DenseCandidateSource<'_> {
    fn name(&self) -> &'static str {
        "dense"
    }

    fn search(&self, store: &dyn GraphStore, repo: &str, query: &str, kind: Option<NodeKind>, top_k: usize) -> Result<Vec<i64>> {
        let embedding = self.embedder.embed(query)?;
        let raw_hits = store.search_similar(repo, &embedding, top_k, kind)?;

        let Some(projector) = self.projector else {
            return Ok(raw_hits.into_iter().map(|(n, _)| n.id).collect());
        };

        let projected_query = projector.project(&embedding);
        let mut reranked: Vec<(i64, f32)> = Vec::with_capacity(raw_hits.len());
        for (node, _distance) in raw_hits {
            // A candidate that was somehow never actually embedded (shouldn't
            // happen -- search_similar only returns embedded nodes -- but
            // not assumed) is skipped rather than crashing the re-rank.
            let Some(raw) = store.get_embedding(repo, node.id)? else { continue };
            let projected = projector.project(&raw);
            reranked.push((node.id, cosine(&projected_query, &projected)));
        }
        reranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(reranked.into_iter().map(|(id, _)| id).collect())
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|v| v * v).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
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
    /// Personalized PageRank mass this node received from
    /// `personalized_pagerank`, `None` when `graph_expand` wasn't
    /// requested (never `Some(0.0)` in that case) -- like the `*_rank`
    /// fields, exposed so a caller/UI can show *why* graph expansion moved
    /// a result, not folded silently into `fused_score`.
    pub graph_score: Option<f64>,
}

/// Standard RRF constant — dampens the influence of any single source's
/// exact rank ordering among its own top results, so one source dominating
/// its own list doesn't overwhelm the fused ranking.
const RRF_K: f32 = 60.0;

/// Relations `personalized_pagerank`'s bounded-neighborhood fetch follows --
/// `Affects`/`References` are this codebase's plasticity-bearing edges (see
/// `agentops_graph::effective_weight`); `DependsOn`/`Documents` are
/// deterministic structural facts with no relevance signal to spread.
const GRAPH_EXPAND_RELATIONS: [EdgeRelation; 2] = [EdgeRelation::Affects, EdgeRelation::References];
/// How many hops `personalized_pagerank`'s bounded-neighborhood fetch
/// explores out from the RRF-fused seed hits.
const GRAPH_EXPAND_DEPTH: u32 = 2;
/// Node cap on that same bounded-neighborhood fetch -- same value and same
/// cost rationale as `agentops-api::subgraph`'s `NODE_CAP` (a blocking
/// round trip per node visited on `PostgresGraphStore`), not coincidence.
const GRAPH_EXPAND_NODE_CAP: usize = 150;
/// HippoRAG 2's damping/restart value for query-time Personalized PageRank
/// over an undirected weighted entity graph.
const PPR_DAMPING: f64 = 0.5;
const PPR_ITERATIONS: usize = 20;

/// Query-time Personalized PageRank over `neighborhood`, restarting to a
/// uniform distribution over whichever `seed_ids` are actually present in
/// it (silently skipping any that fell outside the bounded fetch). Treats
/// edges as undirected -- HippoRAG 2 runs PPR over an undirected weighted
/// entity-passage graph, and this codebase's own `Affects`/`References`
/// edges are meaningful to spread activation across regardless of which
/// direction they happen to be stored in. Edge weight is
/// `agentops_graph::effective_edge_weight` (the existing plasticity/decay
/// substrate, per-relation half-life aware since Initiative 1 gave
/// `References` its own), not a flat `1.0` -- a heavily-reinforced,
/// recently-confirmed edge spreads more activation than a stale,
/// unreinforced one.
///
/// Power iteration only, entirely in memory over the already-fetched
/// `neighborhood` -- see `bounded_neighborhood`'s own doc comment for why
/// this must never re-query the store mid-iteration. `petgraph::algo::
/// page_rank` is not used here: it only supports uniform damping, not a
/// seed-biased restart vector, so this is a small hand-rolled loop instead.
/// A node with zero neighborhood-internal edges (a dangling node in PPR
/// terms) simply stops propagating past its own restart share, rather than
/// redistributing its mass elsewhere -- an acceptable simplification at the
/// bounded scale (`GRAPH_EXPAND_NODE_CAP`) this runs over.
pub fn personalized_pagerank(neighborhood: &agentops_graph::BoundedNeighborhood, seed_ids: &[i64], damping: f64, iterations: usize) -> HashMap<i64, f64> {
    let node_ids: Vec<i64> = neighborhood.nodes.iter().map(|(n, _)| n.id).collect();
    if node_ids.is_empty() {
        return HashMap::new();
    }

    let mut adjacency: HashMap<i64, Vec<(i64, f64)>> = HashMap::new();
    for edge in &neighborhood.edges {
        let w = effective_edge_weight(edge).max(1e-6);
        adjacency.entry(edge.src_id).or_default().push((edge.dst_id, w));
        adjacency.entry(edge.dst_id).or_default().push((edge.src_id, w));
    }

    let present_seeds: Vec<i64> = seed_ids.iter().copied().filter(|id| node_ids.contains(id)).collect();
    if present_seeds.is_empty() {
        return HashMap::new();
    }
    let restart_mass = 1.0 / present_seeds.len() as f64;
    let mut restart: HashMap<i64, f64> = HashMap::new();
    for &s in &present_seeds {
        *restart.entry(s).or_insert(0.0) += restart_mass;
    }

    let mut rank: HashMap<i64, f64> = node_ids.iter().map(|&id| (id, *restart.get(&id).unwrap_or(&0.0))).collect();

    for _ in 0..iterations {
        let mut next: HashMap<i64, f64> = node_ids.iter().map(|&id| (id, (1.0 - damping) * *restart.get(&id).unwrap_or(&0.0))).collect();
        for &u in &node_ids {
            let r_u = *rank.get(&u).unwrap_or(&0.0);
            let Some(neighbors) = adjacency.get(&u) else { continue };
            let out_weight_sum: f64 = neighbors.iter().map(|(_, w)| w).sum();
            if r_u <= 0.0 || out_weight_sum <= 0.0 {
                continue;
            }
            for &(v, w) in neighbors {
                *next.entry(v).or_insert(0.0) += damping * r_u * (w / out_weight_sum);
            }
        }
        rank = next;
    }

    rank
}

/// Recency ranking multiplier (Initiative 3, CLS-inspired retrieval plan) --
/// GENESIS's temporal-embedding recency effect, applied to `search_hybrid`'s
/// node ranking by reusing the exact same decay curve `effective_weight`
/// already applies to `Affects` edges, rather than inventing new decay math.
/// `1.0` (a true no-op, not a penalty) when `node.last_touched_at` is `None`
/// -- `PostgresGraphStore` doesn't populate it yet (see `schema.sql`), so
/// this must never rank a Postgres-backed node lower just because the
/// signal isn't available there.
fn recency_multiplier(node: &Node) -> f32 {
    match &node.last_touched_at {
        Some(ts) => effective_weight(1.0, age_days(ts)) as f32,
        None => 1.0,
    }
}

/// Fuses dense + lexical + exact candidate lists via Reciprocal Rank
/// Fusion: `score(node) = sum over sources containing it of 1 / (RRF_K +
/// rank + 1)`. Each source is over-fetched at `top_k * 3` internally so the
/// fusion has enough candidates to work with even when the final `top_k` is
/// small, then the fused, sorted result is truncated to `top_k`.
///
/// `graph_expand`, if true, runs a second, HippoRAG/HippoRAG-2-inspired
/// pass after fusion: seed a bounded, in-memory Personalized PageRank
/// (`personalized_pagerank`) from the fused top hits and fold each node's
/// PPR mass into a *damped copy* of `fused_score` for the final sort --
/// never the displayed `fused_score` itself, same convention
/// `prominence_rank_multiplier` already established below. Off by default
/// so existing callers/tests see no behavior change unless they opt in.
///
/// `projector`, if given, re-ranks the dense signal through a trained
/// per-repo `EmbeddingProjector` (Initiative 5) -- see that trait's own
/// doc comment for why this crate accepts one rather than loading it
/// itself. `#[allow(clippy::too_many_arguments)]`: bundling these into an
/// options struct would touch every one of this function's many existing
/// call sites (tests, the CLI, the MCP tool) for a signature that's
/// already stable and well-understood -- a pragmatic exception, not an
/// oversight (the same "documented exception over blind compliance"
/// reasoning `deny.toml` already uses elsewhere in this codebase).
#[allow(clippy::too_many_arguments)]
pub fn search_hybrid(store: &dyn GraphStore, embedder: &dyn Embedder, repo: &str, query: &str, top_k: usize, kind: Option<NodeKind>, graph_expand: bool, projector: Option<&dyn EmbeddingProjector>) -> Result<Vec<HybridHit>> {
    let fetch_k = (top_k * 3).max(10);
    let dense = DenseCandidateSource { embedder, projector }.search(store, repo, query, kind, fetch_k)?;
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
        hits.push(HybridHit { node, fused_score, dense_rank, lexical_rank, exact_rank, graph_score: None });
    }

    let graph_scores: HashMap<i64, f64> = if graph_expand && !hits.is_empty() {
        // Seed PPR from the RRF-fused top hits themselves, not raw dense
        // hits alone -- so a lexical/exact-only match can still seed
        // activation, per the plan's revised design.
        let mut seed_order = hits.clone();
        seed_order.sort_by(|a, b| b.fused_score.partial_cmp(&a.fused_score).unwrap_or(std::cmp::Ordering::Equal));
        let seed_ids: Vec<i64> = seed_order.iter().take(fetch_k.min(10)).map(|h| h.node.id).collect();
        let neighborhood = bounded_neighborhood(store, repo, NeighborhoodQuery { seed_ids: &seed_ids, relations: &GRAPH_EXPAND_RELATIONS, direction: TraversalDirection::Both, max_depth: GRAPH_EXPAND_DEPTH, kind_filter: &[], cap: GRAPH_EXPAND_NODE_CAP })?;
        personalized_pagerank(&neighborhood, &seed_ids, PPR_DAMPING, PPR_ITERATIONS)
    } else {
        HashMap::new()
    };
    if graph_expand {
        for hit in &mut hits {
            hit.graph_score = Some(*graph_scores.get(&hit.node.id).unwrap_or(&0.0));
        }
    }

    // Sort by a damped copy of fused_score, not the field itself -- a
    // Reduced-prominence hit ranks lower (this is the MCP semantic_search
    // tool's actual live ranking, the surface an agent sees mid-session)
    // but `HybridHit.fused_score` stays the real RRF value callers display.
    // `graph_score` (when present) contributes as `1.0 + score` so a node
    // with zero PPR mass is a no-op multiplier rather than zeroing out an
    // otherwise-good RRF match. `recency_multiplier` (Initiative 3) reuses
    // the exact same damped-copy convention -- a stale node ranks lower,
    // never its displayed `fused_score`.
    hits.sort_by(|a, b| {
        let rank_a = a.fused_score * agentops_graph::prominence_rank_multiplier(a.node.prominence) as f32 * (1.0 + a.graph_score.unwrap_or(0.0) as f32) * recency_multiplier(&a.node);
        let rank_b = b.fused_score * agentops_graph::prominence_rank_multiplier(b.node.prominence) as f32 * (1.0 + b.graph_score.unwrap_or(0.0) as f32) * recency_multiplier(&b.node);
        rank_b.partial_cmp(&rank_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(top_k);
    Ok(hits)
}

/// How many top gist-tier (`NodeKind::DocSection`) hits seed the detail-tier
/// scope in `search_gist_then_detail`.
const GIST_TOP_K: usize = 3;
/// Hard ceiling on the detail-tier pass's over-fetch `top_k`, regardless of
/// how large a matched section's `scope` is -- see that call site's own
/// comment for the real, live sqlite-vec `k` overflow this guards against.
const DETAIL_PASS_MAX_TOP_K: usize = 1000;

/// Two-tier retrieval (Initiative 2, CLS-inspired retrieval plan): first
/// searches only the "gist"/cortical tier (`NodeKind::DocSection` --
/// `agentops-docgen`'s already-compressed module/repo summaries, indexed by
/// `agentops-mcp::docgen::index_doc_sections`) to shortlist which part of
/// the repo the query is about, then searches the "detail"/hippocampal tier
/// (everything else) restricted to the nodes those matched sections
/// actually cover (`EdgeRelation::Covers`). Falls back to an unscoped
/// `search_hybrid` call if no section matched, or none of the matched
/// sections cover anything (e.g. a prose-only section, or no doc page has
/// been generated for this repo yet) -- coarse-then-fine is a refinement
/// over flat search, never a stricter gate that can return fewer results
/// than plain `search_hybrid` would for the same query.
pub fn search_gist_then_detail(store: &dyn GraphStore, embedder: &dyn Embedder, repo: &str, query: &str, top_k: usize) -> Result<Vec<HybridHit>> {
    let gist_hits = search_hybrid(store, embedder, repo, query, GIST_TOP_K, Some(NodeKind::DocSection), false, None)?;
    if gist_hits.is_empty() {
        return search_hybrid(store, embedder, repo, query, top_k, None, false, None);
    }

    let mut scope: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for hit in &gist_hits {
        for edge in store.edges_from(repo, hit.node.id)? {
            if edge.relation == EdgeRelation::Covers {
                scope.insert(edge.dst_id);
            }
        }
    }
    if scope.is_empty() {
        return search_hybrid(store, embedder, repo, query, top_k, None, false, None);
    }

    // Over-fetch beyond top_k so post-hoc scoping still has enough
    // candidates to fill top_k from, the same over-fetch-then-filter shape
    // `search_hybrid`'s own dense/lexical/exact fan-out already uses --
    // bounded at `DETAIL_PASS_MAX_TOP_K`, well under sqlite-vec's internal
    // KNN `k <= 4096` limit (`search_hybrid` itself further multiplies
    // whatever `top_k` it's given by 3 for its own dense-signal fetch_k, so
    // this must stay comfortably below 4096/3). A real, live bug caught via
    // E2E testing against this repo's own graph.db, not a hypothetical: an
    // unbounded `.max(scope.len())` here hit the sqlite-vec `k` ceiling
    // outright once a matched Core Modules section covered ~1700 of this
    // repo's ~2200 real symbols.
    let detail_hits = search_hybrid(store, embedder, repo, query, (top_k * 3).max(scope.len()).min(DETAIL_PASS_MAX_TOP_K), None, false, None)?;
    let mut scoped: Vec<HybridHit> = detail_hits.into_iter().filter(|h| scope.contains(&h.node.id)).collect();
    scoped.truncate(top_k);
    Ok(scoped)
}

/// Similarity floor (Initiative 4's "interference elimination" guardrail,
/// per CodaRAG's three-stage framing -- Consolidation / Associative
/// Navigation / Interference Elimination) below which a dense-similar
/// candidate is dropped from a pattern-completion bundle rather than
/// included -- prevents gist-collapse of superficially-similar-but-unrelated
/// symbols into recombined context. A deliberately conservative first-pass
/// constant, not derived from anything; needs empirical tuning against real
/// repos (flagged as the initiative's main open risk in the plan this
/// shipped against).
const PATTERN_COMPLETE_SIMILARITY_FLOOR: f32 = 0.5;
/// Same guardrail for graph-connected (PPR) candidates -- a node with
/// vanishingly small PPR mass is graph-adjacent in name only.
const PATTERN_COMPLETE_PPR_FLOOR: f64 = 0.01;
/// `search_similar` over-fetches this multiple of `k` so the similarity
/// floor still has real candidates to filter from.
const PATTERN_COMPLETE_FETCH_MULTIPLE: usize = 3;

/// How a `PatternCompletionMatch` relates back to the pattern-completion
/// seed -- exposed (not folded into one opaque score) so a caller/prompt
/// can say *why* a piece of recombined context was pulled in, the same
/// transparency convention `HybridHit`'s `*_rank`/`graph_score` fields
/// already established.
#[derive(Debug, Clone, Copy)]
pub enum PatternCompletionSource {
    /// Dense cosine similarity to the seed's own content.
    Similar(f32),
    /// Personalized PageRank mass reached from the seed via
    /// `Affects`/`References` edges (Initiative 0).
    Graph(f64),
}

/// One symbol recombined into a pattern-completion bundle around a seed
/// symbol, plus its own `Affects`-derived notes -- the exact `(kind, title,
/// body, prominence, curation_reason)` tuple shape
/// `agentops-llm::build_prompt` already consumes for the seed itself,
/// reused rather than reinvented so `explain_symbol` can fold this straight
/// into its existing prompt-assembly code without a translation layer.
#[derive(Debug, Clone)]
pub struct PatternCompletionMatch {
    pub node: Node,
    pub via: PatternCompletionSource,
    pub notes: Vec<(NodeKind, String, String, agentops_graph::NodeProminence, Option<String>)>,
}

/// Pattern completion around `seed_id` -- GENESIS's episodic recombination
/// (mixing components from multiple stored items produces more useful
/// context than single-source sampling), operationalized here as retrieval/
/// assembly rather than generation, since no generative model exists in
/// this codebase (Initiative 4, CLS-inspired retrieval plan). Finds up to
/// `k` symbols most similar-and-connected to `seed_id`, via dense
/// `search_similar` over the seed's own content plus Initiative 0's
/// Personalized PageRank expansion over `Affects`/`References` edges, each
/// contributing its own `Affects`-derived Gotcha/Decision notes. Candidates
/// below the similarity/PPR floors are dropped (interference elimination) --
/// recombination stays high-precision instead of gist-collapsing unrelated
/// symbols into one bundle. Never includes `seed_id` itself. Returns an
/// empty `Vec` (not an error) if `seed_id` doesn't exist or has no content
/// to embed -- the dense signal simply contributes nothing in that case,
/// same as `search_hybrid`'s per-signal degrade-gracefully convention.
pub fn pattern_complete(store: &dyn GraphStore, embedder: &dyn Embedder, repo: &str, seed_id: i64, k: usize) -> Result<Vec<PatternCompletionMatch>> {
    let Some(seed) = store.get_node(repo, seed_id)? else { return Ok(Vec::new()) };

    let mut candidates: HashMap<i64, PatternCompletionSource> = HashMap::new();

    if let Some(content) = &seed.content {
        let embedding = embedder.embed(content)?;
        for (node, distance) in store.search_similar(repo, &embedding, k * PATTERN_COMPLETE_FETCH_MULTIPLE, Some(NodeKind::Symbol))? {
            if node.id == seed_id {
                continue;
            }
            // Same L2-distance-to-cosine-similarity conversion
            // `agentops-api::search`'s own header comment documents for
            // sqlite-vec's default (L2) distance metric over
            // fastembed's L2-normalized vectors.
            let similarity = 1.0 - (distance.powi(2) / 2.0);
            if similarity >= PATTERN_COMPLETE_SIMILARITY_FLOOR {
                candidates.entry(node.id).or_insert(PatternCompletionSource::Similar(similarity));
            }
        }
    }

    let neighborhood = bounded_neighborhood(store, repo, NeighborhoodQuery { seed_ids: &[seed_id], relations: &GRAPH_EXPAND_RELATIONS, direction: TraversalDirection::Both, max_depth: GRAPH_EXPAND_DEPTH, kind_filter: &[NodeKind::Symbol], cap: GRAPH_EXPAND_NODE_CAP })?;
    let ppr = personalized_pagerank(&neighborhood, &[seed_id], PPR_DAMPING, PPR_ITERATIONS);
    for (id, score) in ppr {
        if id == seed_id || score < PATTERN_COMPLETE_PPR_FLOOR {
            continue;
        }
        candidates.entry(id).or_insert(PatternCompletionSource::Graph(score));
    }

    let mut ordered: Vec<(i64, PatternCompletionSource)> = candidates.into_iter().collect();
    ordered.sort_by(|a, b| {
        let score_a = match a.1 {
            PatternCompletionSource::Similar(s) => s as f64,
            PatternCompletionSource::Graph(s) => s,
        };
        let score_b = match b.1 {
            PatternCompletionSource::Similar(s) => s as f64,
            PatternCompletionSource::Graph(s) => s,
        };
        score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    ordered.truncate(k);

    let mut results = Vec::with_capacity(ordered.len());
    for (id, via) in ordered {
        let Some(node) = store.get_node(repo, id)? else { continue };
        let notes = store
            .edges_to(repo, id)?
            .into_iter()
            .filter(|e| e.relation == EdgeRelation::Affects)
            .filter_map(|e| store.get_node(repo, e.src_id).ok().flatten())
            .filter(|n| matches!(n.kind, NodeKind::Gotcha | NodeKind::Decision))
            .map(|n| (n.kind, n.name.clone().unwrap_or_default(), n.content.clone().unwrap_or_default(), n.prominence, n.curation_reason.clone()))
            .collect();
        results.push(PatternCompletionMatch { node, via, notes });
    }

    Ok(results)
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

    #[test]
    fn personalized_pagerank_propagates_activation_from_the_seed_to_a_connected_neighbor_but_not_an_isolated_node() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let seed = symbol(&store, "demo", "a.rs", "seed", "fn seed() {}");
        let neighbor = symbol(&store, "demo", "b.rs", "neighbor", "fn neighbor() {}");
        let isolated = symbol(&store, "demo", "c.rs", "isolated", "fn isolated() {}");
        store.add_edge("demo", seed, neighbor, EdgeRelation::References).unwrap();

        let neighborhood = bounded_neighborhood(&store, "demo", NeighborhoodQuery { seed_ids: &[seed], relations: &GRAPH_EXPAND_RELATIONS, direction: TraversalDirection::Both, max_depth: GRAPH_EXPAND_DEPTH, kind_filter: &[], cap: GRAPH_EXPAND_NODE_CAP }).unwrap();
        let scores = personalized_pagerank(&neighborhood, &[seed], PPR_DAMPING, PPR_ITERATIONS);

        assert!(scores[&seed] > 0.0, "the seed itself must retain PPR mass from its own restart share");
        assert!(scores[&neighbor] > 0.0, "a directly connected neighbor must receive spread activation");
        assert!(!scores.contains_key(&isolated), "an isolated node outside the bounded neighborhood must not appear at all");
    }

    #[test]
    fn personalized_pagerank_weights_a_reinforced_edge_higher_than_an_unreinforced_one() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let seed = symbol(&store, "demo", "a.rs", "seed", "fn seed() {}");
        let strong = symbol(&store, "demo", "b.rs", "strong", "fn strong() {}");
        let weak = symbol(&store, "demo", "c.rs", "weak", "fn weak() {}");
        let strong_edge = store.add_edge("demo", seed, strong, EdgeRelation::References).unwrap();
        store.add_edge("demo", seed, weak, EdgeRelation::References).unwrap();
        for _ in 0..5 {
            store.reinforce_edge("demo", strong_edge, true).unwrap();
        }

        let neighborhood = bounded_neighborhood(&store, "demo", NeighborhoodQuery { seed_ids: &[seed], relations: &GRAPH_EXPAND_RELATIONS, direction: TraversalDirection::Both, max_depth: GRAPH_EXPAND_DEPTH, kind_filter: &[], cap: GRAPH_EXPAND_NODE_CAP }).unwrap();
        let scores = personalized_pagerank(&neighborhood, &[seed], PPR_DAMPING, PPR_ITERATIONS);

        assert!(scores[&strong] > scores[&weak], "a reinforced (higher-weight) edge must spread more PPR mass than an unreinforced one: {scores:?}");
    }

    #[test]
    fn search_hybrid_graph_expand_false_never_sets_graph_score() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        symbol(&store, "demo", "auth.rs", "verify_token", "fn verify_token() {}");

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, None, false, None).unwrap();
        assert!(hits.iter().all(|h| h.graph_score.is_none()), "graph_score must stay None when graph_expand is off: {hits:?}");
    }

    #[test]
    fn search_hybrid_graph_expand_true_ranks_a_graph_connected_hit_above_an_equally_matched_isolated_one() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let anchor = symbol(&store, "demo", "auth.rs", "verify_token", "fn verify_token() { checks credentials }");
        let connected = symbol(&store, "demo", "auth_helpers.rs", "credential_helper", "fn credential_helper() { checks credentials too }");
        let isolated = symbol(&store, "demo", "unrelated.rs", "other_helper", "fn other_helper() { checks credentials as well }");
        store.add_edge("demo", anchor, connected, EdgeRelation::References).unwrap();
        for (id, content) in [(anchor, "fn verify_token() { checks credentials }"), (connected, "fn credential_helper() { checks credentials too }"), (isolated, "fn other_helper() { checks credentials as well }")] {
            store.set_embedding("demo", id, &LocalEmbedder.embed(content).unwrap()).unwrap();
        }

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "checks credentials", 5, None, true, None).unwrap();
        let connected_hit = hits.iter().find(|h| h.node.id == connected).expect("must appear");
        let isolated_hit = hits.iter().find(|h| h.node.id == isolated).expect("must appear");
        // Every hit here is small enough to also become its own PPR seed, so
        // `isolated` still gets its own restart share -- the real signal
        // `graph_expand` should add is that `connected` gets *more* than
        // that, thanks to the extra activation `anchor`'s edge propagates to
        // it, not that an unconnected node scores exactly zero.
        assert!(
            connected_hit.graph_score.unwrap_or(0.0) > isolated_hit.graph_score.unwrap_or(0.0),
            "a References-connected node must accumulate more PPR mass than an equally text-matched but graph-isolated one: {hits:?}"
        );
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

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token_signature", 5, None, false, None).unwrap();
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

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "expiry logic", 5, None, false, None).unwrap();
        let multi_pos = hits.iter().position(|h| h.node.id == multi).expect("must appear");
        let lexical_only_pos = hits.iter().position(|h| h.node.id == lexical_only).expect("must appear");
        assert!(multi_pos < lexical_only_pos, "a node matched by more signals must fuse to a higher rank: {hits:?}");
    }

    #[test]
    fn search_hybrid_is_repo_scoped() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        symbol(&store, "other-repo", "a.rs", "verify_token", "fn verify_token() {}");

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, None, false, None).unwrap();
        assert!(hits.is_empty(), "a different repo's node must never appear: {hits:?}");
    }

    #[test]
    fn search_hybrid_respects_kind_scoping() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        symbol(&store, "demo", "a.rs", "verify_token", "fn verify_token() {}");
        upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("verify_token".into()), container: None, start_line: None, end_line: None, content: Some("a gotcha about verify_token".into()) }).unwrap();

        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, Some(NodeKind::Symbol), false, None).unwrap();
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
        let score_before = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, None, false, None).unwrap().into_iter().find(|h| h.node.id == reduced).unwrap().fused_score;
        store.set_curation("demo", reduced, agentops_graph::NodeProminence::Reduced, Some("niche")).unwrap();
        let hits = search_hybrid(&store, &LocalEmbedder, "demo", "verify_token", 5, None, false, None).unwrap();
        let reduced_hit = hits.iter().find(|h| h.node.id == reduced).expect("must still appear -- curation never hides a hit");
        assert_eq!(reduced_hit.fused_score, score_before, "the displayed fused_score itself must stay real/undamped, not silently altered by curation");

        let full_pos = hits.iter().position(|h| h.node.id == full).unwrap();
        let reduced_pos = hits.iter().position(|h| h.node.id == reduced).unwrap();
        assert!(full_pos < reduced_pos, "the Reduced-prominence hit must rank lower once curated: {hits:?}");
    }

    fn doc_section(store: &dyn GraphStore, repo: &str, id: &str, content: &str) -> i64 {
        upsert_node(store, NewNode { kind: NodeKind::DocSection, repo: repo.into(), path: Some(format!("doc_section:{id}")), name: Some(id.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap()
    }

    /// Regression test for a real bug caught via E2E testing against this
    /// repo's own live graph.db: an unbounded `.max(scope.len())` on the
    /// detail-tier over-fetch blew past sqlite-vec's internal KNN `k <=
    /// 4096` limit once a matched section covered ~1700 of this repo's real
    /// ~2200 symbols (`search_hybrid`'s own dense fetch_k multiplies
    /// whatever `top_k` it's given by 3, so the actual `k` sent to
    /// sqlite-vec was 3x whatever `scope.len()` was). 1500 covered symbols
    /// here reproduces the same class of overflow if `DETAIL_PASS_MAX_TOP_K`
    /// regresses.
    #[test]
    fn search_gist_then_detail_does_not_overflow_sqlite_vecs_knn_limit_with_a_huge_scope() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let section = doc_section(&store, "demo", "everything", "covers nearly the whole repo verify_token");
        for i in 0..1500 {
            let sym = symbol(&store, "demo", &format!("f{i}.rs"), &format!("sym{i}"), &format!("fn sym{i}() {{}}"));
            store.add_edge("demo", section, sym, EdgeRelation::Covers).unwrap();
        }
        let target = symbol(&store, "demo", "target.rs", "verify_token", "fn verify_token() {}");
        store.add_edge("demo", section, target, EdgeRelation::Covers).unwrap();

        let hits = search_gist_then_detail(&store, &LocalEmbedder, "demo", "verify_token", 5).unwrap();
        assert!(hits.iter().any(|h| h.node.id == target), "{hits:?}");
    }

    #[test]
    fn search_gist_then_detail_scopes_results_to_what_the_matched_section_covers() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let section = doc_section(&store, "demo", "auth", "the auth module covers verify_token");
        let covered = symbol(&store, "demo", "auth.rs", "verify_token", "fn verify_token() {}");
        let uncovered = symbol(&store, "demo", "other.rs", "verify_token_elsewhere", "fn verify_token_elsewhere() {}");
        store.add_edge("demo", section, covered, EdgeRelation::Covers).unwrap();

        let hits = search_gist_then_detail(&store, &LocalEmbedder, "demo", "verify_token", 5).unwrap();
        let ids: std::collections::HashSet<i64> = hits.iter().map(|h| h.node.id).collect();
        assert!(ids.contains(&covered), "the section-covered symbol must appear: {hits:?}");
        assert!(!ids.contains(&uncovered), "a symbol not covered by the matched section must be excluded from the scoped detail pass: {hits:?}");
    }

    #[test]
    fn search_gist_then_detail_falls_back_to_unscoped_when_no_doc_section_matches() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let target = symbol(&store, "demo", "a.rs", "verify_token", "fn verify_token() {}");
        // No DocSection nodes exist at all -- the gist pass must find
        // nothing and this must fall back to a plain, unscoped search
        // rather than returning zero results.

        let hits = search_gist_then_detail(&store, &LocalEmbedder, "demo", "verify_token", 5).unwrap();
        assert!(hits.iter().any(|h| h.node.id == target), "must fall back to unscoped search_hybrid: {hits:?}");
    }

    #[test]
    fn search_gist_then_detail_falls_back_when_the_matched_section_covers_nothing() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        doc_section(&store, "demo", "overview", "verify_token appears here but this section covers no symbols");
        let target = symbol(&store, "demo", "a.rs", "verify_token", "fn verify_token() {}");
        // Deliberately no Covers edge from the section at all.

        let hits = search_gist_then_detail(&store, &LocalEmbedder, "demo", "verify_token", 5).unwrap();
        assert!(hits.iter().any(|h| h.node.id == target), "a matched-but-empty-coverage section must fall back to unscoped search, not return nothing: {hits:?}");
    }

    /// Initiative 3: `recency_multiplier` tested directly and deterministically
    /// rather than end-to-end through `search_hybrid` -- an end-to-end test
    /// needs two otherwise-perfectly-tied hits to isolate recency as the only
    /// variable, but FTS5's bm25 turned out to give the first-inserted of two
    /// identical-content documents a very slightly different score purely
    /// from internal statistics/insertion order (confirmed live: two symbols
    /// with byte-identical content and equal-length names still produced
    /// `fused_score`s of 0.016393 vs 0.016129, not an exact tie) -- too
    /// small and unpredictable a gap to reliably out-rank via real elapsed
    /// time in a fast unit test, and not something worth fighting. Testing
    /// the pure function directly is both simpler and actually deterministic.
    #[test]
    fn recency_multiplier_favors_a_more_recently_touched_node() {
        let mut older = symbol_node_for_test();
        older.last_touched_at = Some(format_test_timestamp(10.0));
        let mut newer = symbol_node_for_test();
        newer.last_touched_at = Some(format_test_timestamp(0.0));

        assert!(recency_multiplier(&newer) > recency_multiplier(&older), "a node touched 0 days ago must score higher than one touched 10 days ago");
    }

    #[test]
    fn recency_multiplier_is_a_no_op_when_last_touched_at_is_absent() {
        let mut node = symbol_node_for_test();
        node.last_touched_at = None;
        assert_eq!(recency_multiplier(&node), 1.0, "PostgresGraphStore doesn't populate last_touched_at yet -- must never penalize a node just because the signal is unavailable there");
    }

    fn symbol_node_for_test() -> Node {
        Node { id: 1, kind: NodeKind::Symbol, repo: "demo".into(), path: None, name: None, container: None, start_line: None, end_line: None, content: None, curated: false, prominence: agentops_graph::NodeProminence::Full, curation_reason: None, last_touched_at: None }
    }

    /// SQLite's `CURRENT_TIMESTAMP` shape ("YYYY-MM-DD HH:MM:SS", UTC) at
    /// `age_days_ago` days before now -- a minimal, dependency-free
    /// formatter for this test only, mirroring `agentops-graph`'s own
    /// `format_unix_for_test` helper for the identical purpose.
    fn format_test_timestamp(age_days_ago: f64) -> String {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as f64;
        let then = (now - age_days_ago * 86_400.0) as u64;
        let days = (then / 86_400) as i64;
        let secs_of_day = then % 86_400;
        let (h, m, s) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m_ = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m_ <= 2 { y + 1 } else { y };
        format!("{y:04}-{m_:02}-{d:02} {h:02}:{m:02}:{s:02}")
    }

    #[test]
    fn pattern_complete_finds_a_graph_connected_symbol_via_references() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let seed = symbol(&store, "demo", "a.rs", "seed", "fn seed() {}");
        let connected = symbol(&store, "demo", "b.rs", "connected", "fn connected() {}");
        store.add_edge("demo", seed, connected, EdgeRelation::References).unwrap();

        let results = pattern_complete(&store, &LocalEmbedder, "demo", seed, 5).unwrap();
        let found = results.iter().find(|m| m.node.id == connected).expect("the References-connected symbol must be found via PPR");
        assert!(matches!(found.via, PatternCompletionSource::Graph(score) if score > 0.0), "{:?}", found.via);
    }

    #[test]
    fn pattern_complete_includes_notes_affecting_a_recombined_symbol() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let seed = symbol(&store, "demo", "a.rs", "seed", "fn seed() {}");
        let connected = symbol(&store, "demo", "b.rs", "connected", "fn connected() {}");
        store.add_edge("demo", seed, connected, EdgeRelation::References).unwrap();
        let gotcha = upsert_node(&store, NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("watch out".into()), container: None, start_line: None, end_line: None, content: Some("edge case here".into()) }).unwrap();
        store.add_edge("demo", gotcha, connected, EdgeRelation::Affects).unwrap();

        let results = pattern_complete(&store, &LocalEmbedder, "demo", seed, 5).unwrap();
        let found = results.iter().find(|m| m.node.id == connected).unwrap();
        assert_eq!(found.notes.len(), 1);
        assert_eq!(found.notes[0].1, "watch out");
    }

    #[test]
    fn pattern_complete_never_includes_the_seed_itself() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let seed = symbol(&store, "demo", "a.rs", "seed", "fn seed() {}");
        let connected = symbol(&store, "demo", "b.rs", "connected", "fn connected() {}");
        store.add_edge("demo", seed, connected, EdgeRelation::References).unwrap();
        store.add_edge("demo", connected, seed, EdgeRelation::References).unwrap();

        let results = pattern_complete(&store, &LocalEmbedder, "demo", seed, 5).unwrap();
        assert!(results.iter().all(|m| m.node.id != seed), "the seed must never recombine with itself, even via a cycle back to it: {results:?}");
    }

    #[test]
    fn pattern_complete_returns_empty_for_an_unknown_seed() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let results = pattern_complete(&store, &LocalEmbedder, "demo", 999_999, 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn pattern_complete_respects_the_k_limit() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let seed = symbol(&store, "demo", "seed.rs", "seed", "fn seed() {}");
        for i in 0..5 {
            let leaf = symbol(&store, "demo", &format!("leaf{i}.rs"), &format!("leaf{i}"), &format!("fn leaf{i}() {{}}"));
            store.add_edge("demo", seed, leaf, EdgeRelation::References).unwrap();
        }

        let results = pattern_complete(&store, &LocalEmbedder, "demo", seed, 2).unwrap();
        assert_eq!(results.len(), 2, "{results:?}");
    }

    #[test]
    fn pattern_complete_excludes_a_graph_connected_symbol_below_the_ppr_floor() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let seed = symbol(&store, "demo", "seed.rs", "seed", "fn seed() {}");
        // A long chain dilutes PPR mass well below PATTERN_COMPLETE_PPR_FLOOR
        // by the far end -- the interference-elimination guardrail this test
        // checks for.
        let mut prev = seed;
        for i in 0..10 {
            let next = symbol(&store, "demo", &format!("s{i}.rs"), &format!("s{i}"), &format!("fn s{i}() {{}}"));
            store.add_edge("demo", prev, next, EdgeRelation::References).unwrap();
            prev = next;
        }
        let far_end = prev;

        let results = pattern_complete(&store, &LocalEmbedder, "demo", seed, 20).unwrap();
        assert!(results.iter().all(|m| m.node.id != far_end), "a symbol with vanishingly small PPR mass must be filtered out, not included as if it were meaningfully related: {results:?}");
    }
}
