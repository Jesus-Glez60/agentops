//! Semantic search over the neuron graph — BGE-M3 dense embeddings (the same
//! model the original codebrain/docbrain used), generated locally via ONNX
//! (`fastembed`, no Python runtime, no external embedding API — code/docs
//! never leave the process to get embedded), indexed into Qdrant.
//!
//! **Why this exists**: `agentops-graph`'s structural retrieval (`list_gotchas`,
//! `repo_map`, exact symbol lookups) is precise but exhaustive — a caller
//! gets everything of a kind and has to read past it to find what's
//! relevant. That costs tokens on every call an agent makes. `SemanticIndex`
//! answers "what's actually relevant to this query" directly, so a RAG step
//! returns the top-k relevant passages instead of the whole graph.

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use qdrant_client::qdrant::point_id::PointIdOptions;
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, Distance, Filter, PointStruct, QueryPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::{Payload, Qdrant};

/// BGE-M3's dense embedding output dimension.
const VECTOR_SIZE: u64 = 1024;

pub struct SemanticIndex {
    model: TextEmbedding,
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

impl SemanticIndex {
    /// Connects to Qdrant at `qdrant_url` (gRPC, e.g. `http://localhost:6334`)
    /// and loads the BGE-M3 ONNX model — downloaded from Hugging Face Hub and
    /// cached locally on first use. This is a real network dependency, but
    /// scoped to the heavy tier (which already talks to Postgres/GitHub/etc.)
    /// — not the light-tier scanner's zero-runtime-network-egress invariant.
    pub fn connect(qdrant_url: &str, collection: &str) -> Result<Self> {
        let model = TextEmbedding::try_new(InitOptions::new(EmbeddingModel::BGEM3).with_show_download_progress(true))
            .context("loading BGE-M3 embedding model")?;
        let client = Qdrant::from_url(qdrant_url).build().context("connecting to Qdrant")?;
        Ok(Self { model, client, collection: collection.to_string() })
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
    pub async fn index_graph_store(&mut self, store: &dyn agentops_graph::GraphStore, repo: &str) -> Result<usize> {
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

        let count = items.len();
        // Batch embedding calls rather than one call per item or one giant
        // call for the whole repo.
        for chunk in items.chunks(64) {
            self.index(chunk).await?;
        }
        Ok(count)
    }

    /// Returns the `top_k` items most semantically similar to `query`,
    /// optionally scoped to one repo.
    pub async fn search(&mut self, query: &str, top_k: u64, repo: Option<&str>) -> Result<Vec<SearchHit>> {
        let embeddings = self.model.embed(vec![query], None).context("embedding query")?;
        let query_vector = embeddings.into_iter().next().context("no embedding produced for the query")?;

        let mut builder = QueryPointsBuilder::new(&self.collection).query(query_vector).limit(top_k).with_payload(true);
        if let Some(repo) = repo {
            builder = builder.filter(Filter::all([Condition::matches("repo", repo.to_string())]));
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
}
