# Retrieval

How a search query is turned into a ranked list of code and knowledge nodes, on each surface and in each mode. Read this before tuning or extending search, or when a result ranks somewhere unexpected.

Crate detail is in [02-crates/knowledge-and-retrieval.md](02-crates/knowledge-and-retrieval.md) and [02-crates/semantic-index-and-consolidation.md](02-crates/semantic-index-and-consolidation.md).

## Retrieval surfaces

| Surface | Function | Modes |
| --- | --- | --- |
| MCP `local_semantic_search` | `tool_semantic_search` (`agentops-core/crates/agentops-mcp/src/tools.rs`) | `dense` (default), `hybrid`, `gist_then_detail` |
| CLI `agentops search` | `agentops-core/crates/agentops-cli/src/main.rs` | dense; `--hybrid`; `--hybrid --graph-expand` |
| REST `GET /local-search` (web Search page) | `agentops-api/src/search.rs` (single-operator), `agentops-heavy-api/src/dashboard.rs` (hosted) | dense only (`search_similar`) |
| MCP `related_context` | `agentops_retrieval::pattern_complete` | dense + graph, around one symbol |
| MCP `semantic_search` (stdio, Qdrant) | `SemanticIndex::search` (`agentops-heavy-embeddings`) | BGE-M3 + cross-encoder rerank |
| REST `GET /search` (hosted) | `search_query_handler` (`agentops-heavy-api/src/lib.rs`) | same Qdrant path; HTTP 503 (`search_not_configured`) without `AGENTOPS_QDRANT_URL` |
| MCP `get_symbol` | exact lookup | returns the symbol with the gotchas and decisions that affect it |

Only the MCP tool and the CLI expose hybrid search. The web Search page is dense-only. On hosted repos that matters, because hosted indexing writes no core embeddings (`findings.md` F28).

## Retrieval modes

| Mode | What it does | Uses projector | Uses recency | Uses prominence |
| --- | --- | --- | --- | --- |
| `dense` | `search_similar` over embedded nodes only, re-sorted by prominence | no | no | yes |
| `hybrid` | `search_hybrid`: dense + lexical + exact, fused by RRF, optional graph expansion | yes, if one is promoted | yes | yes |
| `gist_then_detail` | `search_gist_then_detail`: match `DocSection` nodes first, then `search_hybrid` inside what they cover | as `hybrid` | as `hybrid` | as `hybrid` |

`dense` returns "No matches" when nothing is embedded. Embedding is opt-in (`with_embeddings`; see [04-indexing-pipeline.md](04-indexing-pipeline.md), "Embeddings"). `hybrid` still finds unembedded nodes through its lexical and exact signals.

The `kind` filter applies to `dense` and `hybrid`. `gist_then_detail` ignores it (handler comment).

## Candidate sources

`CandidateSource` returns node ids already ordered best-first (`agentops-core/crates/agentops-retrieval/src/lib.rs`). Fusion uses only rank positions, because the raw scores are not comparable: cosine distance, SQLite `bm25`, Postgres `ts_rank` and a fixed exact score use different scales (trait doc comment).

| Source | Store method | SQLite | Postgres |
| --- | --- | --- | --- |
| `DenseCandidateSource` | `search_similar` | sqlite-vec KNN on `node_vectors` | pgvector on `nodes.embedding` |
| `LexicalCandidateSource` | `search_lexical` | FTS5 `MATCH`, ordered by `bm25` | `tsvector`, ordered by `ts_rank` |
| `ExactCandidateSource` | `search_exact` | case-insensitive name equality, then substring (`LOWER(name) LIKE`); exact matches first, then shorter names | equivalent query |

Store-level details (`agentops-core/crates/agentops-graph/src/sqlite.rs`):

- The lexical query quotes each whitespace-separated term as its own phrase and ORs them (`fts5_match_query`). Any punctuation is safe, and the search is "any term matches".
- Dense and lexical search over-fetch (`top_k * 8`, at least 50), then filter by repo and kind in Rust. When `kind` names a rare kind, matches can fall outside the over-fetch window and be missed.
- sqlite-vec rejects `k > 4096`. `search_similar` clamps at that limit itself, so callers stacking multipliers cannot overflow it (gotcha 54092).

### Projection re-rank

If the repo has a promoted projection head, `DenseCandidateSource` re-ranks the raw KNN candidates by cosine similarity in the projected space. It does not query a second index; the projection only re-orders what raw search returned (doc comment). The head is loaded by `agentops_mcp::consolidate::load_active_projector`, used in `hybrid` mode only. Training is in [06-notes-and-consolidation.md](06-notes-and-consolidation.md).

## Retrieval fusion

`search_hybrid(store, embedder, repo, query, top_k, kind, graph_expand, projector)`:

1. Each source is queried with `fetch_k = max(top_k * 3, 10)`.
2. Every candidate gets a reciprocal-rank-fusion score summed over the sources that returned it:

   `fused_score = Σ 1 / (RRF_K + rank + 1)`, with `RRF_K = 60` and 0-based `rank`.

3. Optional graph expansion adds a `graph_score` (next section).
4. Hits are sorted by a **sort key**, not by `fused_score`:

   `fused_score × prominence_rank_multiplier × (1 + graph_score) × recency_multiplier` (computed as `rank_a`/`rank_b` in the sort closure)

5. The list is truncated to `top_k`. Expanded hits get a citation path.

`fused_score` stays the real RRF value in output. Each multiplier changes only the sort key, so a displayed score is never adjusted (doc comment on `search_hybrid`; `prominence_rank_multiplier`).

| Multiplier | Value |
| --- | --- |
| Prominence | `Full` 1.0, `Pinned` 1.0, `Reduced` 0.1 |
| Graph | `1 + graph_score`; 1.0 when expansion is off or the node got no PageRank mass |
| Recency | `0.5 ^ (age_days / 30)` from the node's `last_touched_at` (`effective_weight`, `AFFECTS_EDGE_HALF_LIFE_DAYS`). 1.0 when `last_touched_at` is `None`, which is always the case on Postgres. |

Each hit carries `dense_rank`, `lexical_rank`, `exact_rank` (`None` if that source missed it) and `graph_score`, so callers can show why it ranked where it did. The MCP output prints them as `signals: dense+lexical+exact`, plus `graph`, a reduced-prominence warning with its reason, and a `via:` citation trail.

> Recency halves a node's rank weight every 30 days without edits, including stable code that matters. The half-life is shared with `Affects` edges rather than chosen for nodes. Whether that is intended is not recorded; see `questions.md` Q12.

## Graph expansion

With `graph_expand: true` (MCP `hybrid` mode, CLI `--hybrid --graph-expand`):

1. The top `min(fetch_k, 10)` fused hits become seeds. Seeding from fused hits, not dense hits only, lets a lexical-only or exact-only match seed activation.
2. `bounded_neighborhood` fetches the graph around the seeds: relations `Affects` and `References` only, both directions, depth `GRAPH_EXPAND_DEPTH = 2`, at most `GRAPH_EXPAND_NODE_CAP = 150` nodes.
3. `personalized_pagerank` runs on that neighborhood, treating edges as undirected and weighted by effective (decayed) weight. Damping `PPR_DAMPING = 0.5`, `PPR_ITERATIONS = 20`, restart to a uniform distribution over the seeds.
4. Each hit's `graph_score` is its PageRank mass.
5. For final hits with mass, `shortest_path` reconstructs the chain of edges from a seed, which is printed as the citation.

`DependsOn` and `Documents` are excluded because they are "deterministic structural facts with no relevance signal to spread" (`GRAPH_EXPAND_RELATIONS` comment). The 150-node cap matches the subgraph endpoint's `NODE_CAP`, because each visited node costs a blocking round trip on Postgres.

The damping and iteration values come from HippoRAG 2's query-time PageRank (constant doc comment). Path citations were added in `4a3cd12`.

## Gist-then-detail

`search_gist_then_detail(store, embedder, repo, query, top_k)`:

1. Search only `DocSection` nodes (the generated doc sections; see [04-indexing-pipeline.md](04-indexing-pipeline.md), "Repo state and documentation"). Take the top `GIST_TOP_K = 3`.
2. Collect the nodes those sections cover (`Covers` edges).
3. Run `search_hybrid` restricted to that scope, with an over-fetch capped at `DETAIL_PASS_MAX_TOP_K = 1000`.
4. If no section matched, or matched sections cover nothing, fall back to plain `search_hybrid`.

The fallback means this mode never returns fewer results than flat search would (doc comment). Doc sections exist only after a scan has generated the doc page. They are embedded only when the scan used `with_embeddings`.

## Prominence dampening

A `Reduced` gotcha ranks ×0.1 in `dense`, `hybrid` and `gist_then_detail`, and every result shows "⚠ reduced prominence — <reason>". The decision "Gotchas are curated, never resolved/dismissed -- don't reintroduce a bug-tracker model" lists the other surfaces it reaches: `list_gotchas`, `get_symbol`, `explain_symbol`'s prompt and docgen. Hosted `/local-search` applies it too (`dashboard.rs`); the single-operator `/local-search` in `agentops-api/src/search.rs` does not. In `dense` mode the distance is divided by the multiplier for sorting only; the printed distance is unchanged.

## Pattern completion

`related_context(path, symbol_id, top_k, compact)` calls `pattern_complete(store, embedder, repo, seed_id, k)`. It finds symbols related to a seed symbol and returns each one with the gotchas and decisions attached to it through `Affects` edges.

| Signal | How | Floor |
| --- | --- | --- |
| Dense | `search_similar` on the seed's own content, over-fetching `PATTERN_COMPLETE_FETCH_MULTIPLE = 3` × `k` | cosine similarity ≥ `PATTERN_COMPLETE_SIMILARITY_FLOOR = 0.5` |
| Graph | PageRank from the seed over `Affects`/`References` | mass ≥ `PATTERN_COMPLETE_PPR_FLOOR = 0.01` |

Candidates below the floors are dropped ("interference elimination"), so unrelated look-alikes are not mixed into the bundle. The seed itself is never included. A missing seed or one without content returns an empty list, not an error.

The 0.5 floor is "a deliberately conservative first-pass constant, not derived from anything; needs empirical tuning against real repos" (constant doc comment).

`explain_symbol` folds the same bundle into its prompt. `compact: true` returns note titles and ids only; full bodies come from `fetch_content`.

## Semantic index path (Qdrant)

Available only from the stdio `agentops-mcp-server` with `AGENTOPS_QDRANT_URL` set. It is not exposed on hosted `/mcp` (see [07-mcp-and-rest-contracts.md](07-mcp-and-rest-contracts.md)).

| Step | Detail |
| --- | --- |
| Index | `semantic_index` / hosted indexing stage 7: `collect_index_items` → `SemanticIndex::index_items`. Vectors are 1024-dimensional (`VECTOR_SIZE`). |
| Stage 1 | BGE-M3 bi-encoder KNN over a pool of `max(top_k × CANDIDATE_POOL_MULTIPLIER, MIN_CANDIDATE_POOL)` = `max(top_k × 4, 20)` candidates. |
| Stage 2 | `bge-reranker-v2-m3` cross-encoder scores query and candidate together; the top `top_k` are kept. |
| Scope | `search_scoped` filters by `repo`/`kind`. Qdrant has no tenant isolation (`agentops-heavy-api` module doc). |

Both models run locally through `fastembed`/ONNX and are downloaded once from Hugging Face. The 4× pool is "a standard, reasonable middle ground — not tuned against this specific corpus" (constant doc comment).

Core hybrid search has no cross-encoder stage. Its RRF fusion is the final ranking (`agentops-retrieval` module doc).

## Tuning knobs

Per call:

| Knob | Where | Default |
| --- | --- | --- |
| `top_k` | `local_semantic_search`, CLI | 10 (MCP) |
| `kind` | `local_semantic_search` (6 kinds), CLI | none |
| `mode` | `local_semantic_search` | `dense` |
| `graph_expand` | `local_semantic_search` (`hybrid`), CLI | off |
| `max_chars`, `compact` | `get_symbol`, `related_context` | `DEFAULT_CHAR_BUDGET = 4000`; long content is paged through `fetch_content` |

Compile-time constants (change in code):

| Constant | Value | File |
| --- | --- | --- |
| `RRF_K` | 60 | `agentops-retrieval/src/lib.rs` |
| `GRAPH_EXPAND_DEPTH`, `GRAPH_EXPAND_NODE_CAP` | 2, 150 | same |
| `PPR_DAMPING`, `PPR_ITERATIONS` | 0.5, 20 | same |
| `GIST_TOP_K`, `DETAIL_PASS_MAX_TOP_K` | 3, 1000 | same |
| `PATTERN_COMPLETE_SIMILARITY_FLOOR`, `_PPR_FLOOR`, `_FETCH_MULTIPLE` | 0.5, 0.01, 3 | same |
| `AFFECTS_EDGE_HALF_LIFE_DAYS`, `REFERENCES_EDGE_HALF_LIFE_DAYS` | 30, 90 | `agentops-graph/src/lib.rs` |
| `prominence_rank_multiplier` (`Reduced`) | 0.1 | same |
| `CANDIDATE_POOL_MULTIPLIER`, `MIN_CANDIDATE_POOL` | 4, 20 | `agentops-heavy-embeddings/src/lib.rs` |

Environment: `AGENTOPS_QDRANT_URL`, `AGENTOPS_QDRANT_COLLECTION` enable the Qdrant path. No env var changes core ranking.

## Open questions

- Q12: recency half-life on nodes.
- Related findings: F19 (two node kinds not filterable), F28 (hosted web search).
