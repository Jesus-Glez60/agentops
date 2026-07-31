//! Semantic search over the neuron graph — BGE-M3 dense embeddings (the same
//! model the original codebrain/docbrain used), generated locally via ONNX
//! (`fastembed`, no Python runtime, no external embedding API — code/docs
//! never leave the process to get embedded), indexed into Qdrant, with a
//! second-stage bge-reranker-v2-m3 cross-encoder pass — again, the same
//! model the original system used (there via `sentence_transformers.CrossEncoder`,
//! here via `fastembed`'s own `TextRerank`, which ships this exact model).
//!
//! **Why this exists**: `agentops-graph`'s structural retrieval (`list_gotchas`,
//! `repo_map`, exact symbol lookups) is precise but exhaustive — a caller
//! gets everything of a kind and has to read past it to find what's
//! relevant. That costs tokens on every call an agent makes. `SemanticIndex`
//! answers "what's actually relevant to this query" directly, so a RAG step
//! returns the top-k relevant passages instead of the whole graph.
//!
//! **Why two stages, not just embeddings**: a bi-encoder (BGE-M3) embeds the
//! query and each document independently, so it's fast enough to search a
//! whole corpus but only approximates relevance. A cross-encoder (the
//! reranker) sees the query and a candidate document *together* in one
//! forward pass, which is slower — too slow to run over an entire corpus —
//! but meaningfully more accurate. The standard pattern, and the one this
//! follows: use the embedding search to cheaply pull a wide candidate pool,
//! then spend the expensive cross-encoder pass only on that shortlist.

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, RerankInitOptions, RerankerModel, TextEmbedding, TextRerank};
use qdrant_client::qdrant::point_id::PointIdOptions;
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, Distance, Filter, PointStruct, QueryPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::{Payload, Qdrant};

/// BGE-M3's dense embedding output dimension.
const VECTOR_SIZE: u64 = 1024;

/// How much wider than the caller's requested `top_k` the first-stage
/// (embedding) candidate pool is, before reranking narrows it back down.
/// Reranking a too-small pool defeats the point (the bi-encoder's mistakes
/// never get a chance to be corrected); rereanking the whole corpus is the
/// slow thing this two-stage design exists to avoid. 4x is a standard,
/// reasonable middle ground — not tuned against this specific corpus, but a
/// documented, deliberate default rather than an arbitrary one.
const CANDIDATE_POOL_MULTIPLIER: u64 = 4;
const MIN_CANDIDATE_POOL: u64 = 20;

pub struct SemanticIndex {
    model: TextEmbedding,
    reranker: TextRerank,
    client: Qdrant,
    collection: String,
}

/// One unit of content to embed and index — typically one `agentops-graph`
/// node's content (a symbol's source, a gotcha's text, a doc chunk).
#[derive(Debug, Clone)]
pub struct IndexItem {
    /// Stable identifier — reuse the source graph node's id, so re-indexing
    /// a rescanned repo overwrites the prior point instead of duplicating it.
    pub id: u64,
    pub text: String,
    pub repo: String,
    pub kind: String,
    pub name: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: u64,
    pub score: f32,
    pub repo: String,
    pub kind: String,
    pub name: Option<String>,
    pub path: Option<String>,
    pub text: String,
}

/// Reads every Symbol/Gotcha/Decision node out of `store` into embeddable
/// `IndexItem`s — File nodes are skipped, since they have no meaningful
/// text of their own (their symbols already carry that). Deliberately a
/// plain synchronous function, not a method on `SemanticIndex`: it takes
/// `&dyn GraphStore`, which isn't provably `Sync` (`SqliteGraphStore` wraps
/// a `rusqlite::Connection`, intentionally `!Sync` upstream), so a
/// reference to it must never be held across an `.await` — keeping this
/// synchronous and separate from `SemanticIndex::index_items` (the actual
/// async embedding step) is what makes that impossible to get wrong by
/// accident.
pub fn collect_index_items(store: &dyn agentops_graph::GraphStore, repo: &str) -> Result<Vec<IndexItem>> {
    use agentops_graph::NodeKind;

    let mut items = Vec::new();
    for kind in [NodeKind::Symbol, NodeKind::Gotcha, NodeKind::Decision] {
        for node in store.nodes_by_kind(kind).context("reading nodes to index")? {
            let Some(content) = &node.content else { continue };
            let text = match node.name.as_deref() {
                Some(name) => format!("{name}\n\n{content}"),
                None => content.clone(),
            };
            items.push(IndexItem {
                id: node.id as u64,
                text,
                repo: repo.to_string(),
                kind: kind.as_str().to_string(),
                name: node.name.clone(),
                path: node.path.clone(),
            });
        }
    }
    Ok(items)
}

/// Docbrain `DocNode.id` and `agentops-graph` node `id` are independent
/// autoincrement sequences from two entirely different SQLite databases —
/// nothing stops a docbrain doc node and a codebrain symbol node from
/// sharing the same numeric id. Since both get cast to `u64` and used as
/// Qdrant point ids in the same collection (see `collect_doc_index_items`'s
/// doc comment on why they share one), an unnamespaced collision would mean
/// one silently overwrites the other on upsert. Setting the top bit on
/// docbrain ids reserves the two halves of the u64 space for the two
/// sources — real ids are always non-negative `i64`s (< 2^63), so the two
/// ranges can never overlap.
const DOC_ID_NAMESPACE_BIT: u64 = 1 << 63;

/// Reads every doc node for `slug` (across all versions — see
/// `DocbrainStore::all_doc_nodes`'s doc comment on why exact-version scoping
/// doesn't apply here) into embeddable `IndexItem`s, so `search_docs` has
/// real docbrain content to find. `kind` is set to `"doc"` — distinct from
/// `collect_index_items`'s `"symbol"`/`"gotcha"`/`"decision"` — so the two
/// corpora can share one Qdrant collection (avoiding a second ~2GB model
/// load in the same process) while `SemanticIndex::search`'s optional `kind`
/// filter keeps a codebrain symbol search from ever surfacing a docbrain doc
/// section or vice versa. `repo` is repurposed to hold the library slug —
/// same field, same filtering mechanism `vector_search` already has, just a
/// different corpus using it as the scope key.
///
/// Deliberately synchronous and separate from any async indexing step, for
/// the same reason `collect_index_items` is: `DocbrainStore` wraps a
/// `rusqlite::Connection`, which is `!Sync`, so a reference to it must never
/// be held across an `.await`.
pub fn collect_doc_index_items(store: &docbrain_graph::DocbrainStore, tenant: &docbrain_graph::TenantContext, slug: &str) -> Result<Vec<IndexItem>> {
    let nodes = store.all_doc_nodes(tenant, slug).context("reading doc nodes to index")?;
    Ok(nodes
        .into_iter()
        .map(|node| IndexItem {
            id: (node.id as u64) | DOC_ID_NAMESPACE_BIT,
            text: format!("{}\n\n{}", node.topic, node.content),
            repo: slug.to_string(),
            kind: "doc".to_string(),
            name: Some(node.topic),
            path: Some(node.version),
        })
        .collect())
}

impl SemanticIndex {
    /// Connects to Qdrant at `qdrant_url` (gRPC, e.g. `http://localhost:6334`)
    /// and loads both ONNX models — BGE-M3 (embeddings) and bge-reranker-v2-m3
    /// (reranking) — downloaded from Hugging Face Hub and cached locally on
    /// first use. This is a real network dependency, but scoped to the heavy
    /// tier (which already talks to Postgres/GitHub/etc.) — not the
    /// light-tier scanner's zero-runtime-network-egress invariant.
    pub fn connect(qdrant_url: &str, collection: &str) -> Result<Self> {
        let model = TextEmbedding::try_new(InitOptions::new(EmbeddingModel::BGEM3).with_show_download_progress(true))
            .context("loading BGE-M3 embedding model")?;
        let reranker = TextRerank::try_new(RerankInitOptions::new(RerankerModel::BGERerankerV2M3).with_show_download_progress(true))
            .context("loading bge-reranker-v2-m3 reranking model")?;
        let client = Qdrant::from_url(qdrant_url).build().context("connecting to Qdrant")?;
        Ok(Self { model, reranker, client, collection: collection.to_string() })
    }

    /// Creates the collection if it doesn't already exist. Safe to call on
    /// every startup.
    pub async fn ensure_collection(&self) -> Result<()> {
        if self.client.collection_exists(&self.collection).await.context("checking Qdrant collection")? {
            return Ok(());
        }
        self.client
            .create_collection(CreateCollectionBuilder::new(&self.collection).vectors_config(VectorParamsBuilder::new(VECTOR_SIZE, Distance::Cosine)))
            .await
            .context("creating Qdrant collection")?;
        Ok(())
    }

    /// Embeds and upserts `items` in one batch. Re-indexing the same `id`
    /// overwrites the prior point (Qdrant upsert semantics) — safe to call
    /// repeatedly as a repo is rescanned.
    pub async fn index(&mut self, items: &[IndexItem]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
        let embeddings = self.model.embed(texts, None).context("generating embeddings")?;

        let mut points = Vec::with_capacity(items.len());
        for (item, vector) in items.iter().zip(embeddings) {
            let payload: Payload = serde_json::json!({
                "repo": item.repo,
                "kind": item.kind,
                "name": item.name,
                "path": item.path,
                "text": item.text,
            })
            .try_into()
            .context("building point payload")?;
            points.push(PointStruct::new(item.id, vector, payload));
        }

        self.client.upsert_points(UpsertPointsBuilder::new(&self.collection, points)).await.context("upserting points into Qdrant")?;
        Ok(())
    }

    /// Indexes every Symbol/Gotcha/Decision node from `store` — File nodes
    /// are skipped, since they have no meaningful text of their own (their
    /// symbols already carry that). Node ids are reused as Qdrant point
    /// ids, so re-running this after a rescan overwrites stale entries
    /// instead of duplicating them. Returns the number of nodes indexed.
    ///
    /// Deliberately takes owned `items` (via `collect_index_items`, called
    /// separately by the caller) rather than `&dyn GraphStore` directly:
    /// `&dyn GraphStore` isn't provably `Sync` (SQLite connections aren't),
    /// so a reference to it can't cross an `.await` point — an async method
    /// on this type taking it directly would produce a `!Send` future,
    /// which silently breaks any caller (like an axum handler) that needs
    /// this to run on a multi-threaded executor. Splitting the synchronous
    /// graph read from the async embedding step avoids the trap entirely.
    pub async fn index_items(&mut self, items: &[IndexItem]) -> Result<usize> {
        let count = items.len();
        // Batch embedding calls rather than one call per item or one giant
        // call for the whole repo.
        for chunk in items.chunks(64) {
            self.index(chunk).await?;
        }
        Ok(count)
    }

    /// Returns the `top_k` items most relevant to `query`, optionally scoped
    /// to one repo — a two-stage search: a wide embedding-based recall pass
    /// (see `vector_search`) narrowed down by the bge-reranker-v2-m3
    /// cross-encoder, which is slower but more accurate than embedding
    /// similarity alone since it scores the query and each candidate
    /// document together rather than comparing independently-computed
    /// vectors.
    pub async fn search(&mut self, query: &str, top_k: u64, repo: Option<&str>) -> Result<Vec<SearchHit>> {
        self.search_scoped(query, top_k, repo, None).await
    }

    /// Same two-stage pipeline as `search`, plus an optional `kind` filter —
    /// used to keep two corpora sharing one collection (codebrain's
    /// `"symbol"/"gotcha"/"decision"` nodes and docbrain's `"doc"` nodes,
    /// see `collect_doc_index_items`) from ever surfacing in each other's
    /// results, without needing a second Qdrant collection or a second
    /// (expensive, ~2GB) model load.
    pub async fn search_scoped(&mut self, query: &str, top_k: u64, repo: Option<&str>, kind: Option<&str>) -> Result<Vec<SearchHit>> {
        let candidate_pool = (top_k * CANDIDATE_POOL_MULTIPLIER).max(MIN_CANDIDATE_POOL);
        let candidates = self.vector_search_scoped(query, candidate_pool, repo, kind).await?;
        if candidates.is_empty() {
            return Ok(candidates);
        }

        let documents: Vec<&str> = candidates.iter().map(|c| c.text.as_str()).collect();
        let reranked = self.reranker.rerank(query, documents, false, None).context("reranking candidates")?;

        Ok(reranked
            .into_iter()
            .take(top_k as usize)
            .map(|r| {
                let mut hit = candidates[r.index].clone();
                hit.score = r.score;
                hit
            })
            .collect())
    }

    /// The first stage on its own: embedding-based cosine-similarity search
    /// against Qdrant, unreranked. `search` is almost always what callers
    /// actually want; this is exposed mainly for testing the recall stage
    /// in isolation from the (slower) reranking stage.
    pub async fn vector_search(&mut self, query: &str, top_k: u64, repo: Option<&str>) -> Result<Vec<SearchHit>> {
        self.vector_search_scoped(query, top_k, repo, None).await
    }

    /// Same as `vector_search`, plus an optional `kind` filter — see
    /// `search_scoped`.
    pub async fn vector_search_scoped(&mut self, query: &str, top_k: u64, repo: Option<&str>, kind: Option<&str>) -> Result<Vec<SearchHit>> {
        let embeddings = self.model.embed(vec![query], None).context("embedding query")?;
        let query_vector = embeddings.into_iter().next().context("no embedding produced for the query")?;

        let mut builder = QueryPointsBuilder::new(&self.collection).query(query_vector).limit(top_k).with_payload(true);
        let mut conditions = Vec::new();
        if let Some(repo) = repo {
            conditions.push(Condition::matches("repo", repo.to_string()));
        }
        if let Some(kind) = kind {
            conditions.push(Condition::matches("kind", kind.to_string()));
        }
        if !conditions.is_empty() {
            builder = builder.filter(Filter::all(conditions));
        }

        let result = self.client.query(builder).await.context("querying Qdrant")?;
        result.result.into_iter().map(point_to_hit).collect()
    }
}

fn point_to_hit(mut point: qdrant_client::qdrant::ScoredPoint) -> Result<SearchHit> {
    let id = match point.id.and_then(|id| id.point_id_options) {
        Some(PointIdOptions::Num(n)) => n,
        other => anyhow::bail!("expected a numeric point id, got {other:?}"),
    };
    let get_str = |payload: &mut std::collections::HashMap<String, qdrant_client::qdrant::Value>, key: &str| -> Option<String> {
        payload.remove(key).and_then(|v| v.as_str().map(|s| s.to_string()))
    };

    Ok(SearchHit {
        id,
        score: point.score,
        repo: get_str(&mut point.payload, "repo").unwrap_or_default(),
        kind: get_str(&mut point.payload, "kind").unwrap_or_default(),
        name: get_str(&mut point.payload, "name"),
        path: get_str(&mut point.payload, "path"),
        text: get_str(&mut point.payload, "text").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hits a REAL Qdrant instance (the docker-compose stack in
    /// agentops-heavy/docker/) and downloads the REAL BGE-M3 ONNX model on
    /// first run. Set AGENTOPS_TEST_QDRANT_URL to run; skipped otherwise,
    /// since not every environment has Docker or wants a multi-hundred-MB
    /// model download during `cargo test`.
    fn test_index(collection: &str) -> Option<SemanticIndex> {
        let url = std::env::var("AGENTOPS_TEST_QDRANT_URL").ok()?;
        Some(SemanticIndex::connect(&url, collection).expect("connect to test Qdrant + load BGE-M3"))
    }

    #[tokio::test]
    async fn semantically_related_content_ranks_above_unrelated_content() {
        let Some(mut index) = test_index("agentops_test_semantic") else { return };
        index.ensure_collection().await.unwrap();

        let items = vec![
            IndexItem {
                id: 1,
                text: "Pin the SSH host key when connecting over git+ssh so a man-in-the-middle can't intercept the deploy key handshake".into(),
                repo: "test-repo".into(),
                kind: "gotcha".into(),
                name: Some("ssh-host-key-pinning".into()),
                path: None,
            },
            IndexItem {
                id: 2,
                text: "Bake a loaf of sourdough bread by folding the dough every thirty minutes during bulk fermentation".into(),
                repo: "test-repo".into(),
                kind: "gotcha".into(),
                name: Some("sourdough-recipe".into()),
                path: None,
            },
        ];
        index.index(&items).await.unwrap();

        // Real semantic query — no keyword overlap with item 1's text at all
        // ("host key" vs "SSH connection security"), so a keyword/exact-match
        // search would find nothing; a real embedding model should still
        // rank the SSH item above the bread recipe.
        let hits = index.search("SSH connection security", 2, Some("test-repo")).await.unwrap();
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].name.as_deref(), Some("ssh-host-key-pinning"), "expected the SSH gotcha to rank first: {hits:?}");
        assert!(hits[0].score > hits[1].score);
    }

    #[tokio::test]
    async fn repo_filter_excludes_other_repos() {
        let Some(mut index) = test_index("agentops_test_semantic_filter") else { return };
        index.ensure_collection().await.unwrap();

        index
            .index(&[
                IndexItem { id: 10, text: "authentication token verification logic".into(), repo: "repo-a".into(), kind: "symbol".into(), name: Some("verify_token".into()), path: None },
                IndexItem { id: 11, text: "authentication token verification logic".into(), repo: "repo-b".into(), kind: "symbol".into(), name: Some("verify_token".into()), path: None },
            ])
            .await
            .unwrap();

        let hits = index.search("token verification", 5, Some("repo-a")).await.unwrap();
        assert!(hits.iter().all(|h| h.repo == "repo-a"), "{hits:?}");
    }

    #[tokio::test]
    async fn search_scores_are_reranker_scores_not_raw_cosine_similarity() {
        let Some(mut index) = test_index("agentops_test_semantic_rerank") else { return };
        index.ensure_collection().await.unwrap();

        index
            .index(&[IndexItem {
                id: 20,
                text: "Retry with exponential backoff when the upstream API returns HTTP 429.".into(),
                repo: "rerank-test".into(),
                kind: "gotcha".into(),
                name: Some("rate-limit-backoff".into()),
                path: None,
            }])
            .await
            .unwrap();

        let vector_hits = index.vector_search("what to do on a 429 response", 1, Some("rerank-test")).await.unwrap();
        let reranked_hits = index.search("what to do on a 429 response", 1, Some("rerank-test")).await.unwrap();

        assert_eq!(vector_hits.len(), 1);
        assert_eq!(reranked_hits.len(), 1);
        // Cosine similarity from vector_search is bounded to roughly [-1, 1];
        // bge-reranker-v2-m3's cross-encoder score is a raw sigmoid logit
        // that isn't bounded the same way. If these two ever ended up
        // numerically identical it would mean reranking silently didn't run
        // (e.g. a no-op stub), not that the two stages happen to agree.
        assert_ne!(
            vector_hits[0].score, reranked_hits[0].score,
            "search()'s score must come from the reranker, not be a passthrough of vector_search()'s cosine score"
        );
    }

    #[tokio::test]
    async fn vector_search_works_standalone_without_reranking() {
        let Some(mut index) = test_index("agentops_test_semantic_vector_only") else { return };
        index.ensure_collection().await.unwrap();

        index
            .index(&[IndexItem { id: 30, text: "Use constant-time comparison for secrets to avoid timing attacks.".into(), repo: "vs-test".into(), kind: "gotcha".into(), name: Some("timing-safe-compare".into()), path: None }])
            .await
            .unwrap();

        let hits = index.vector_search("how do we compare secrets safely", 1, Some("vs-test")).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name.as_deref(), Some("timing-safe-compare"));
    }

    #[test]
    fn docbrain_and_graph_ids_never_collide_in_the_shared_id_space() {
        // node.id=5 from a codebrain graph.db and DocNode.id=5 from a totally
        // separate docbrain.db must not become the same Qdrant point id.
        let graph_item_id: u64 = 5;
        let doc_item_id: u64 = 5 | DOC_ID_NAMESPACE_BIT;
        assert_ne!(graph_item_id, doc_item_id);
        assert_eq!(doc_item_id & DOC_ID_NAMESPACE_BIT, DOC_ID_NAMESPACE_BIT);
    }

    #[tokio::test]
    async fn kind_filter_keeps_docbrain_docs_and_codebrain_symbols_from_leaking_into_each_others_results() {
        let Some(mut index) = test_index("agentops_test_semantic_kind_filter") else { return };
        index.ensure_collection().await.unwrap();

        index
            .index(&[
                IndexItem {
                    id: 100,
                    text: "React useEffect cleanup function overview".into(),
                    repo: "react".into(),
                    kind: "doc".into(),
                    name: Some("useEffect".into()),
                    path: Some("18.2.0".into()),
                },
                IndexItem {
                    id: 101,
                    text: "React useEffect cleanup function overview".into(),
                    repo: "react".into(),
                    kind: "symbol".into(),
                    name: Some("useEffect".into()),
                    path: None,
                },
            ])
            .await
            .unwrap();

        let doc_hits = index.vector_search_scoped("useEffect cleanup", 5, Some("react"), Some("doc")).await.unwrap();
        assert!(doc_hits.iter().all(|h| h.kind == "doc"), "{doc_hits:?}");

        let symbol_hits = index.vector_search_scoped("useEffect cleanup", 5, Some("react"), Some("symbol")).await.unwrap();
        assert!(symbol_hits.iter().all(|h| h.kind == "symbol"), "{symbol_hits:?}");
    }

    // Hits a real, in-memory DocbrainStore (no network) — verifies
    // collect_doc_index_items reads real doc_nodes and applies the id
    // namespace bit, without needing a live Qdrant instance for this part.
    #[test]
    fn collect_doc_index_items_reads_real_doc_nodes_and_namespaces_ids() {
        use docbrain_graph::{DocbrainStore, TenantContext, Visibility};

        let store = DocbrainStore::open_in_memory().unwrap();
        let tenant = TenantContext::public();
        store.add_library(&tenant, "next", "Next.js", None, Some("https://nextjs.org/docs"), Visibility::Public).unwrap();
        store.add_doc_node(&tenant, "next", "15.1.3", "routing", "App router basics").unwrap();

        let items = collect_doc_index_items(&store, &tenant, "next").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "doc");
        assert_eq!(items[0].repo, "next");
        assert_eq!(items[0].name.as_deref(), Some("routing"));
        assert_eq!(items[0].path.as_deref(), Some("15.1.3"));
        assert!(items[0].id & DOC_ID_NAMESPACE_BIT != 0, "doc item ids must be namespaced");
    }
}
