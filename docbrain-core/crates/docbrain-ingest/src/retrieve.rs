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

static BPE: std::sync::LazyLock<tiktoken_rs::CoreBPE> = std::sync::LazyLock::new(|| tiktoken_rs::cl100k_base().expect("cl100k_base's bundled ranks must load"));

/// Caps a stored node's topic: nodes ingested before `cap_topic` existed
/// can carry a multi-MB topic until re-scraped.
fn bounded(mut node: DocNode) -> DocNode {
    node.topic = crate::chunk::cap_topic(&node.topic);
    node
}

/// A node's budget cost. Floored at chars/4 rather than trusting the stored
/// `token_count` alone: a live `search_docs(max_tokens: 1000)` once returned
/// two ~3.9M-char nodes because a stored count understated them.
fn cost_of(node: &DocNode) -> usize {
    (node.token_count.max(0) as usize).max(node.content.len() / 4)
}

/// Truncates `node`'s content to `budget` tokens, appending a marker. Only a
/// prefix (~8 chars/token, generous) is ever encoded, so a multi-MB node
/// costs no more to cut than a small one.
fn truncate_node(mut node: DocNode, budget: usize) -> DocNode {
    let total_chars = node.content.len();
    let prefix_end = node.content.char_indices().nth(budget.saturating_mul(8)).map_or(total_chars, |(i, _)| i);
    let tokens = BPE.encode_with_special_tokens(&node.content[..prefix_end]);
    let kept = BPE.decode(&tokens[..tokens.len().min(budget)]).unwrap_or_default();
    node.content = format!("{kept}\n… [truncated: node {} is {total_chars} chars — narrow the query]", node.id);
    node.token_count = budget as i64;
    node
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
        let cost = cost_of(&node);
        if used + cost > max_tokens {
            // An oversized first node is cut to fit rather than returned
            // whole (or not at all).
            if used == 0 {
                docs.push(DocResult { node: truncate_node(bounded(node), max_tokens), distance: None, related_via: None });
            }
            break;
        }
        used += cost;
        docs.push(DocResult { node: bounded(node), distance: None, related_via: None });
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
        let cost = cost_of(&hit.node);
        if used + cost > max_tokens {
            if used == 0 {
                docs.push(DocResult { node: truncate_node(bounded(hit.node), max_tokens), distance: Some(hit.distance), related_via: None });
            }
            break;
        }
        used += cost;
        let node_id = hit.node.id;
        docs.push(DocResult { node: bounded(hit.node), distance: Some(hit.distance), related_via: None });

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
            let neighbor_cost = cost_of(&neighbor);
            if used + neighbor_cost > max_tokens {
                continue;
            }
            used += neighbor_cost;
            included.insert(neighbor_id);
            docs.push(DocResult { node: bounded(neighbor), distance: None, related_via: Some(edge.relation) });
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
                examples.push(bounded(node));
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
        store.add_library("next", "Next.js", None, None, None).unwrap();
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
        store.add_library("next", "Next.js", None, None, None).unwrap();
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
        store.add_library("next", "Next.js", None, None, None).unwrap();
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
    fn search_docs_and_get_docs_cap_an_oversized_node_with_an_understated_token_count() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None, None).unwrap();
        let vector = vec![1.0_f32; docbrain_graph::EMBEDDING_DIM];
        let (big, small) = ("word ".repeat(200_000), "a small neighbour");
        let (big_hash, small_hash) = (content_hash(&big), content_hash(small));
        // token_count understates the 1M-char node, as on the live server.
        let big_id = store
            .upsert_doc_node("next", NewDocNode { version: "1.0", topic: "big", content: &big, content_hash: &big_hash, token_count: 10, embedding: Some(&vector), kind: NodeKind::Prose })
            .unwrap()
            .node_id();
        let small_id = store
            .upsert_doc_node("next", NewDocNode { version: "1.0", topic: "small", content: small, content_hash: &small_hash, token_count: 10, embedding: None, kind: NodeKind::Prose })
            .unwrap()
            .node_id();
        store.add_doc_edge(big_id, small_id, EdgeRelation::Sequence).unwrap();

        let docs = search_docs(&store, &FAKE_EMBED, "anything", None, 3, 1000, false).unwrap();
        let chars: usize = docs.iter().map(|d| d.node.content.len()).sum();
        assert!(chars < 8_000, "search_docs returned {chars} chars for max_tokens 1000");
        assert!(docs[0].node.content.contains("[truncated: node"));

        let GetDocsOutcome::Found { docs, .. } = get_docs(&store, "next", "1.0", 1000, false).unwrap() else { panic!("expected Found") };
        let chars: usize = docs.iter().map(|d| d.node.content.len()).sum();
        assert!(chars < 8_000, "get_docs returned {chars} chars for max_tokens 1000");
    }

    #[test]
    fn search_docs_caps_a_legacy_multi_megabyte_topic() {
        // The real live shape: small, correctly-counted content under a
        // ~3.9M-char topic stored before ingest capped topics.
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None, None).unwrap();
        let vector = vec![1.0_f32; docbrain_graph::EMBEDDING_DIM];
        let topic = "Migrating ".repeat(400_000);
        let hash = content_hash("small body");
        store
            .upsert_doc_node("next", NewDocNode { version: "1.0", topic: &topic, content: "small body", content_hash: &hash, token_count: 3, embedding: Some(&vector), kind: NodeKind::Prose })
            .unwrap();

        let docs = search_docs(&store, &FAKE_EMBED, "anything", None, 3, 1000, false).unwrap();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].node.topic.chars().count() <= 201, "topic is {} chars", docs[0].node.topic.len());
    }

    #[test]
    fn get_code_examples_returns_only_has_example_targets() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None, None).unwrap();
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
