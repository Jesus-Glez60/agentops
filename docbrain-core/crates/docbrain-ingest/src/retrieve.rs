//! Retrieval use cases: token-budgeted, edge-expanded reads over the doc
//! graph. Kept out of `docbrain-mcp`'s tool handlers deliberately — the
//! hexagonal architecture guide's rule for inbound (driving) adapters is
//! "parse, call a use case, format the result; no business logic belongs
//! here." This module is that use case, so a future second driving adapter
//! (e.g. `docbrain-api`'s REST routes) can reuse the exact same retrieval
//! behavior instead of re-implementing the embed/KNN/edge-walk/budget
//! logic a second time.

use agentops_embeddings::Embedder;
use anyhow::Result;
use docbrain_graph::{DocNode, DocbrainStore, EdgeRelation, NodeKind};

/// One node included in a retrieval result, with retrieval-specific
/// metadata a formatter might want: `distance` for a direct semantic hit,
/// or `related_via` for a node pulled in by edge-walk from one.
pub struct DocResult {
    pub node: DocNode,
    pub distance: Option<f32>,
    pub related_via: Option<EdgeRelation>,
}

pub enum GetDocsOutcome {
    NotFound { available_versions: Vec<String> },
    Found { docs: Vec<DocResult>, hidden_examples: usize },
}

/// Docs for `slug`@`version`, up to `max_tokens` — never an unbounded full
/// dump. Excludes `CodeExample` nodes unless `include_examples`.
pub fn get_docs(store: &dyn DocbrainStore, slug: &str, version: &str, max_tokens: usize, include_examples: bool) -> Result<GetDocsOutcome> {
    let all_nodes = store.get_doc_nodes(slug, version)?;
    if all_nodes.is_empty() {
        return Ok(GetDocsOutcome::NotFound { available_versions: store.list_doc_versions(slug)? });
    }
    let hidden_examples = if include_examples { 0 } else { all_nodes.iter().filter(|n| n.kind == NodeKind::CodeExample).count() };
    let candidates: Vec<_> = all_nodes.into_iter().filter(|n| include_examples || n.kind == NodeKind::Prose).collect();

    let mut used = 0usize;
    let mut docs = Vec::new();
    for node in candidates {
        let cost = node.token_count as usize;
        if used > 0 && used + cost > max_tokens {
            break;
        }
        used += cost;
        docs.push(DocResult { node, distance: None, related_via: None });
    }
    Ok(GetDocsOutcome::Found { docs, hidden_examples })
}

/// Semantic search with associative synapse-edge expansion, token-budgeted
/// — never a whole page, only what fits. Excludes `CodeExample` nodes, both
/// as direct hits and as edge-walked neighbors, unless `include_examples`:
/// the planning/implementation split (see `chunk.rs`'s module doc).
pub fn search_docs(
    store: &dyn DocbrainStore,
    embed: &dyn Embedder,
    query: &str,
    slug: Option<&str>,
    top_k: usize,
    max_tokens: usize,
    include_examples: bool,
) -> Result<Vec<DocResult>> {
    let exclude_kind = if include_examples { None } else { Some(NodeKind::CodeExample) };
    let embedding = embed.embed(query)?;
    let hits = store.search_similar(&embedding, top_k, slug, exclude_kind)?;

    let mut used = 0usize;
    let mut included = std::collections::BTreeSet::new();
    let mut docs = Vec::new();

    for hit in hits {
        if !included.insert(hit.node.id) {
            continue;
        }
        let cost = hit.node.token_count as usize;
        if used > 0 && used + cost > max_tokens {
            break;
        }
        used += cost;
        let node_id = hit.node.id;
        docs.push(DocResult { node: hit.node, distance: Some(hit.distance), related_via: None });

        // Associative expansion: pull directly-connected synapse-edge
        // context for this hit, budget permitting — this is what makes
        // retrieval more than a flat top-k of isolated chunks.
        for edge in store.node_edges(node_id)? {
            let neighbor_id = if edge.from_node == node_id { edge.to_node } else { edge.from_node };
            if included.contains(&neighbor_id) {
                continue;
            }
            let Some(neighbor) = store.get_node(neighbor_id)? else { continue };
            if !include_examples && neighbor.kind == NodeKind::CodeExample {
                continue;
            }
            let neighbor_cost = neighbor.token_count as usize;
            if used + neighbor_cost > max_tokens {
                continue;
            }
            used += neighbor_cost;
            included.insert(neighbor_id);
            docs.push(DocResult { node: neighbor, distance: None, related_via: Some(edge.relation) });
        }
    }

    Ok(docs)
}

/// The implementation-stage counterpart to a planning-stage `search_docs`
/// call: every `CodeExample` node linked from `node_id` via `HasExample`.
pub fn get_code_examples(store: &dyn DocbrainStore, node_id: i64) -> Result<Vec<DocNode>> {
    anyhow::ensure!(store.get_node(node_id)?.is_some(), "no node {node_id}");
    let mut examples = Vec::new();
    for edge in store.node_edges(node_id)? {
        if matches!(edge.relation, EdgeRelation::HasExample) && edge.from_node == node_id {
            if let Some(node) = store.get_node(edge.to_node)? {
                examples.push(node);
            }
        }
    }
    Ok(examples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use docbrain_graph::{content_hash, NewDocNode, SqliteDocbrainStore};

    struct FakeEmbedder;
    impl Embedder for FakeEmbedder {
        fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![0.1_f32; docbrain_graph::EMBEDDING_DIM])
        }
    }
    const FAKE_EMBED: FakeEmbedder = FakeEmbedder;

    #[test]
    fn get_docs_reports_not_found_with_available_versions() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None).unwrap();
        store.add_doc_snapshot("next", "15.0").unwrap();

        let outcome = get_docs(&store, "next", "16.0", 2000, false).unwrap();
        match outcome {
            GetDocsOutcome::NotFound { available_versions } => assert_eq!(available_versions, vec!["15.0".to_string()]),
            GetDocsOutcome::Found { .. } => panic!("expected NotFound"),
        }
    }

    #[test]
    fn get_docs_excludes_code_examples_by_default() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None).unwrap();
        let prose_hash = content_hash("prose");
        let code_hash = content_hash("code");
        store
            .upsert_doc_node("next", NewDocNode { version: "1.0", topic: "prose", content: "prose", content_hash: &prose_hash, token_count: 1, embedding: None, kind: NodeKind::Prose })
            .unwrap();
        store
            .upsert_doc_node(
                "next",
                NewDocNode { version: "1.0", topic: "code", content: "code", content_hash: &code_hash, token_count: 1, embedding: None, kind: NodeKind::CodeExample },
            )
            .unwrap();

        match get_docs(&store, "next", "1.0", 2000, false).unwrap() {
            GetDocsOutcome::Found { docs, hidden_examples } => {
                assert_eq!(docs.len(), 1);
                assert_eq!(hidden_examples, 1);
            }
            GetDocsOutcome::NotFound { .. } => panic!("expected Found"),
        }
    }

    #[test]
    fn search_docs_excludes_code_examples_from_hits_and_edge_walk_by_default() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None).unwrap();
        let vector = vec![1.0_f32; docbrain_graph::EMBEDDING_DIM];
        let prose_hash = content_hash("prose");
        let code_hash = content_hash("code");
        let prose_id = store
            .upsert_doc_node(
                "next",
                NewDocNode { version: "1.0", topic: "prose", content: "prose", content_hash: &prose_hash, token_count: 1, embedding: Some(&vector), kind: NodeKind::Prose },
            )
            .unwrap()
            .node_id();
        let code_id = store
            .upsert_doc_node(
                "next",
                NewDocNode {
                    version: "1.0",
                    topic: "code",
                    content: "code",
                    content_hash: &code_hash,
                    token_count: 1,
                    embedding: Some(&vector),
                    kind: NodeKind::CodeExample,
                },
            )
            .unwrap()
            .node_id();
        store.add_doc_edge(prose_id, code_id, EdgeRelation::HasExample).unwrap();

        let docs = search_docs(&store, &FAKE_EMBED, "anything", None, 5, 2000, false).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].node.kind, NodeKind::Prose);

        let with_examples = search_docs(&store, &FAKE_EMBED, "anything", None, 5, 2000, true).unwrap();
        assert_eq!(with_examples.len(), 2);
    }

    #[test]
    fn get_code_examples_returns_only_has_example_targets() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None).unwrap();
        let a_hash = content_hash("a");
        let b_hash = content_hash("b");
        let a = store.upsert_doc_node("next", NewDocNode { version: "1.0", topic: "a", content: "a", content_hash: &a_hash, token_count: 1, embedding: None, kind: NodeKind::Prose }).unwrap().node_id();
        let b = store
            .upsert_doc_node("next", NewDocNode { version: "1.0", topic: "b", content: "b", content_hash: &b_hash, token_count: 1, embedding: None, kind: NodeKind::CodeExample })
            .unwrap()
            .node_id();
        store.add_doc_edge(a, b, EdgeRelation::Sequence).unwrap(); // not HasExample — must not be returned

        let examples = get_code_examples(&store, a).unwrap();
        assert!(examples.is_empty(), "a Sequence edge must not count as a code example link");
    }
}
