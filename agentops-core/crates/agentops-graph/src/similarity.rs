//! Similarity edges between memories (milestone 2 of the cue-driven memory
//! work, ADR 0051). A memory that names no symbol has no structural edge,
//! so graph recall from a cue can never reach it. A `SimilarTo` edge links
//! it to the memories it reads most like: the associative link in human
//! memory, one memory bringing up another because they're alike.
//!
//! Edges are computed only here, at write time, where the embedding model
//! has already run (`add_note`, note ingest, the importer, the backfill).
//! Hooks only follow them (`agentops_retrieval::recall_seed`), so recall
//! stays free of embedding work.
//!
//! Similarity is cosine, computed in Rust from the stored vectors: the
//! SQLite store's KNN distance is L2 and Postgres's is cosine, so a score
//! read from either store's search would mean different things.

use anyhow::Result;

use crate::{effective_edge_weight, EdgeRelation, GraphStore, Node, NodeKind};

/// Neighbours linked per write: enough to reach a cluster in one hop.
pub const SIMILAR_K: usize = 5;
/// Cosine at or above which two memories are related, not merely on the
/// same topic. A starting value; the milestone 2 pilot reports the score
/// distribution it was tuned from.
pub const SIMILAR_MIN_SCORE: f32 = 0.75;
/// Most `SimilarTo` edges one memory keeps, so a generic note can't become
/// a hub. Over the cap, the weakest edge (by decayed weight) goes.
pub const SIMILAR_MAX_DEGREE: usize = 12;
/// Cosine at or above which a new memory is a near-copy of an existing one:
/// the importer reinforces the existing memory instead of writing another.
pub const DUPLICATE_MIN_SCORE: f32 = 0.92;

/// The memory kinds that get similarity edges. Doc sections are excluded:
/// they're generated digests of many notes and already the main source of
/// recall noise (finding F53).
pub const MEMORY_KINDS: &[NodeKind] = &[NodeKind::Gotcha, NodeKind::Decision, NodeKind::Note];

/// Cosine similarity, 0 for a zero vector or mismatched lengths.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// The memories most similar to `embedding`, best first, with their cosine.
/// `exclude` drops one node (the memory itself). Fetches a few candidates
/// per kind through the store's own index and re-scores them by cosine.
pub fn similar_memories(store: &dyn GraphStore, repo: &str, embedding: &[f32], exclude: Option<i64>, k: usize) -> Result<Vec<(Node, f32)>> {
    let mut scored = Vec::new();
    for &kind in MEMORY_KINDS {
        for (node, _) in store.search_similar(repo, embedding, k + 1, Some(kind))? {
            if Some(node.id) == exclude {
                continue;
            }
            if let Some(vector) = store.get_embedding(repo, node.id)? {
                let score = cosine(embedding, &vector);
                scored.push((node, score));
            }
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.id.cmp(&b.0.id)));
    scored.truncate(k);
    Ok(scored)
}

/// What one `link_similar` call did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct LinkSummary {
    /// Neighbours linked, with their cosine.
    pub linked: Vec<(i64, f32)>,
    pub edges_created: usize,
    pub edges_reinforced: usize,
    pub edges_dropped: usize,
}

/// Links `node_id` to its `SIMILAR_K` most similar memories above
/// `SIMILAR_MIN_SCORE`, in both directions. A pair already linked is
/// reinforced, not reset. Then every node touched is held to
/// `SIMILAR_MAX_DEGREE`. A node with no embedding, or one that isn't a
/// memory kind, gets nothing. For an explicit write: `add_note`, note
/// ingest, the importer.
pub fn link_similar(store: &dyn GraphStore, repo: &str, node_id: i64) -> Result<LinkSummary> {
    link(store, repo, node_id, true)
}

/// `link_similar` for every embedded memory in the repo. `reinforce: false`
/// only creates missing edges and leaves existing ones as they are: a
/// rescan re-reads every note, and that alone shouldn't strengthen an
/// association (the same rule as `References` edges on rescan).
pub fn link_all_similar(store: &dyn GraphStore, repo: &str, reinforce: bool) -> Result<LinkSummary> {
    let mut total = LinkSummary::default();
    for &kind in MEMORY_KINDS {
        for node in store.nodes_by_kind(repo, kind)? {
            let one = link(store, repo, node.id, reinforce)?;
            total.linked.extend(one.linked);
            total.edges_created += one.edges_created;
            total.edges_reinforced += one.edges_reinforced;
            total.edges_dropped += one.edges_dropped;
        }
    }
    Ok(total)
}

fn link(store: &dyn GraphStore, repo: &str, node_id: i64, reinforce: bool) -> Result<LinkSummary> {
    let mut summary = LinkSummary::default();
    let Some(node) = store.get_node(repo, node_id)? else { return Ok(summary) };
    if !MEMORY_KINDS.contains(&node.kind) {
        return Ok(summary);
    }
    let Some(embedding) = store.get_embedding(repo, node_id)? else { return Ok(summary) };
    let neighbours: Vec<(Node, f32)> = similar_memories(store, repo, &embedding, Some(node_id), SIMILAR_K)?.into_iter().filter(|(_, score)| *score >= SIMILAR_MIN_SCORE).collect();
    for (neighbour, score) in &neighbours {
        for (src, dst) in [(node_id, neighbour.id), (neighbour.id, node_id)] {
            match link_one(store, repo, src, dst, reinforce)? {
                Linked::Created => summary.edges_created += 1,
                Linked::Reinforced => summary.edges_reinforced += 1,
                Linked::Kept => {}
            }
        }
        summary.linked.push((neighbour.id, *score));
    }
    for id in std::iter::once(node_id).chain(neighbours.iter().map(|(n, _)| n.id)) {
        summary.edges_dropped += hold_degree(store, repo, id)?;
    }
    Ok(summary)
}

enum Linked {
    Created,
    Reinforced,
    Kept,
}

/// Creates `src -> dst`, or reinforces it when it exists and `reinforce`.
fn link_one(store: &dyn GraphStore, repo: &str, src: i64, dst: i64, reinforce: bool) -> Result<Linked> {
    if let Some(edge) = store.edges_from(repo, src)?.into_iter().find(|e| e.relation == EdgeRelation::SimilarTo && e.dst_id == dst) {
        if !reinforce {
            return Ok(Linked::Kept);
        }
        store.reinforce_edge(repo, edge.id, true)?;
        return Ok(Linked::Reinforced);
    }
    store.add_edge(repo, src, dst, EdgeRelation::SimilarTo)?;
    Ok(Linked::Created)
}

/// Drops `id`'s weakest `SimilarTo` edges (both directions of each pair)
/// until it has at most `SIMILAR_MAX_DEGREE`. Returns how many it dropped.
fn hold_degree(store: &dyn GraphStore, repo: &str, id: i64) -> Result<usize> {
    let mut edges: Vec<_> = store.edges_from(repo, id)?.into_iter().filter(|e| e.relation == EdgeRelation::SimilarTo).collect();
    if edges.len() <= SIMILAR_MAX_DEGREE {
        return Ok(0);
    }
    edges.sort_by(|a, b| effective_edge_weight(a).total_cmp(&effective_edge_weight(b)).then(a.id.cmp(&b.id)));
    let excess = edges.len() - SIMILAR_MAX_DEGREE;
    for edge in edges.into_iter().take(excess) {
        store.delete_edge(repo, edge.id)?;
        if let Some(back) = store.edges_from(repo, edge.dst_id)?.into_iter().find(|e| e.relation == EdgeRelation::SimilarTo && e.dst_id == id) {
            store.delete_edge(repo, back.id)?;
        }
    }
    Ok(excess)
}

/// Cosine of every distinct pair among each memory's `per_node` nearest
/// neighbours: the distribution `SIMILAR_MIN_SCORE` and
/// `DUPLICATE_MIN_SCORE` are tuned from (milestone 2 pilot report).
pub fn candidate_pair_scores(store: &dyn GraphStore, repo: &str, per_node: usize) -> Result<Vec<f32>> {
    let mut seen = std::collections::HashSet::new();
    let mut scores = Vec::new();
    for &kind in MEMORY_KINDS {
        for node in store.nodes_by_kind(repo, kind)? {
            let Some(embedding) = store.get_embedding(repo, node.id)? else { continue };
            for (other, score) in similar_memories(store, repo, &embedding, Some(node.id), per_node)? {
                if seen.insert((node.id.min(other.id), node.id.max(other.id))) {
                    scores.push(score);
                }
            }
        }
    }
    Ok(scores)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{upsert_node, NewNode, SqliteGraphStore};

    /// A 384-dim vector pointing mostly along `axis`, tilted toward `tilt`
    /// by `amount` (0 = identical direction, larger = less similar).
    fn vector(axis: usize, tilt: usize, amount: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 384];
        v[axis] = 1.0;
        v[tilt] += amount;
        v
    }

    fn memory(store: &SqliteGraphStore, kind: NodeKind, name: &str, embedding: &[f32]) -> i64 {
        let id = upsert_node(store, NewNode { kind, repo: "r".into(), path: Some(format!("vault:{name}.md")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(name.into()) }).unwrap();
        store.set_embedding("r", id, embedding).unwrap();
        id
    }

    fn similar_to(store: &SqliteGraphStore, id: i64) -> Vec<i64> {
        let mut ids: Vec<i64> = store.edges_from("r", id).unwrap().into_iter().filter(|e| e.relation == EdgeRelation::SimilarTo).map(|e| e.dst_id).collect();
        ids.sort();
        ids
    }

    #[test]
    fn candidate_pairs_are_counted_once() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        memory(&store, NodeKind::Note, "a", &vector(0, 1, 0.0));
        memory(&store, NodeKind::Gotcha, "b", &vector(0, 1, 0.1));
        memory(&store, NodeKind::Decision, "c", &vector(0, 1, 0.2));
        assert_eq!(candidate_pair_scores(&store, "r", 5).unwrap().len(), 3);
    }

    #[test]
    fn cosine_is_one_for_the_same_direction_and_zero_when_orthogonal() {
        assert!((cosine(&[1.0, 2.0], &[2.0, 4.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn a_write_links_at_most_k_neighbours_both_ways_above_the_threshold_and_never_a_doc_section() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let new = memory(&store, NodeKind::Note, "new", &vector(0, 1, 0.0));
        let close: Vec<i64> = (0..7).map(|i| memory(&store, [NodeKind::Gotcha, NodeKind::Decision, NodeKind::Note][i % 3], &format!("close{i}"), &vector(0, 2 + i, 0.1 * (i as f32 + 1.0)))).collect();
        let far = memory(&store, NodeKind::Gotcha, "far", &vector(5, 6, 0.0));
        let section = memory(&store, NodeKind::DocSection, "digest", &vector(0, 1, 0.0));

        let summary = link_similar(&store, "r", new).unwrap();
        assert_eq!(summary.linked.len(), SIMILAR_K, "{summary:?}");
        assert_eq!(summary.edges_created, 2 * SIMILAR_K);
        let linked = similar_to(&store, new);
        assert_eq!(linked, close[..SIMILAR_K].to_vec(), "the closest K, best first by cosine");
        assert!(!linked.contains(&far) && !linked.contains(&section));
        for id in &linked {
            assert_eq!(similar_to(&store, *id), vec![new], "stored both ways");
        }
    }

    #[test]
    fn writing_again_reinforces_instead_of_resetting() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = memory(&store, NodeKind::Note, "a", &vector(0, 1, 0.0));
        let b = memory(&store, NodeKind::Gotcha, "b", &vector(0, 1, 0.1));
        link_similar(&store, "r", a).unwrap();
        let again = link_similar(&store, "r", a).unwrap();
        assert_eq!((again.edges_created, again.edges_reinforced), (0, 2));
        let edge = store.edges_from("r", a).unwrap().into_iter().find(|e| e.dst_id == b).unwrap();
        assert!(edge.weight > 1.0, "{edge:?}");
    }

    #[test]
    fn a_rescan_creates_missing_edges_but_leaves_existing_weights_alone() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = memory(&store, NodeKind::Note, "a", &vector(0, 1, 0.0));
        let b = memory(&store, NodeKind::Gotcha, "b", &vector(0, 1, 0.1));
        let first = link_all_similar(&store, "r", false).unwrap();
        assert_eq!(first.edges_created, 2);
        let rescan = link_all_similar(&store, "r", false).unwrap();
        assert_eq!((rescan.edges_created, rescan.edges_reinforced), (0, 0));
        assert_eq!(store.edges_from("r", a).unwrap().into_iter().find(|e| e.dst_id == b).unwrap().weight, 1.0);
    }

    #[test]
    fn no_node_keeps_more_than_the_max_degree() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let hub = memory(&store, NodeKind::Note, "hub", &vector(0, 1, 0.0));
        let others: Vec<i64> = (0..20).map(|i| memory(&store, NodeKind::Gotcha, &format!("m{i}"), &vector(0, 2 + i, 0.05))).collect();
        for id in &others {
            link_similar(&store, "r", *id).unwrap();
        }
        assert!(similar_to(&store, hub).len() <= SIMILAR_MAX_DEGREE);
        for id in others.iter().chain([&hub]) {
            assert!(similar_to(&store, *id).len() <= SIMILAR_MAX_DEGREE, "node {id}");
        }
    }

    #[test]
    fn a_node_with_no_embedding_or_that_isnt_a_memory_gets_nothing() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        memory(&store, NodeKind::Note, "a", &vector(0, 1, 0.0));
        let bare = upsert_node(&store, NewNode { kind: NodeKind::Note, repo: "r".into(), path: Some("vault:bare.md".into()), name: Some("bare".into()), container: None, start_line: None, end_line: None, content: None }).unwrap();
        let section = memory(&store, NodeKind::DocSection, "digest", &vector(0, 1, 0.0));
        assert_eq!(link_similar(&store, "r", bare).unwrap(), LinkSummary::default());
        assert_eq!(link_similar(&store, "r", section).unwrap(), LinkSummary::default());
    }
}
