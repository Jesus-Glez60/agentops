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
/// same topic. Milestone 2 started at 0.75, which linked 77% of candidate
/// pairs (F56); 0.82 keeps roughly the top quarter (milestone 3a).
pub const SIMILAR_MIN_SCORE: f32 = 0.82;
/// Fewest `SimilarTo` edges a memory keeps when a neighbour at or above
/// `SIMILAR_FLOOR_SCORE` exists, so raising the threshold leaves no memory
/// without an association it had a reasonable claim to.
pub const SIMILAR_MIN_EDGES: usize = 1;
/// The old threshold, used only to keep a memory from becoming unlinked.
pub const SIMILAR_FLOOR_SCORE: f32 = 0.75;
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

/// `link_similar` without reinforcing: creates `node_id`'s missing edges
/// and leaves existing ones as they are. For a memory that just got its
/// first vector outside an explicit write (the hosted reindex).
pub fn link_new(store: &dyn GraphStore, repo: &str, node_id: i64) -> Result<LinkSummary> {
    link(store, repo, node_id, false)
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
    let nearest = similar_memories(store, repo, &embedding, Some(node_id), SIMILAR_K)?;
    let mut neighbours: Vec<(Node, f32)> = nearest.iter().filter(|(_, score)| *score >= SIMILAR_MIN_SCORE).cloned().collect();
    // Nothing above the threshold: keep the closest above the floor, so a
    // memory that has a reasonable neighbour isn't left unlinked.
    if neighbours.len() < SIMILAR_MIN_EDGES && similar_degree(store, repo, node_id)? < SIMILAR_MIN_EDGES {
        neighbours.extend(nearest.into_iter().filter(|(_, score)| *score >= SIMILAR_FLOOR_SCORE).take(SIMILAR_MIN_EDGES - neighbours.len()));
    }
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

fn similar_degree(store: &dyn GraphStore, repo: &str, id: i64) -> Result<usize> {
    Ok(store.edges_from(repo, id)?.iter().filter(|e| e.relation == EdgeRelation::SimilarTo).count())
}

/// What `prune_similar` did, and the unlinked share around it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PruneSummary {
    pub memories: usize,
    /// Undirected pairs deleted (two edges each).
    pub pairs_pruned: usize,
    /// Pairs under `SIMILAR_MIN_SCORE` kept or added as a memory's floor.
    pub floor_pairs: usize,
    /// Memories with no edge of any kind, before and after.
    pub unlinked_before: usize,
    pub unlinked_after: usize,
}

/// Raises existing `SimilarTo` edges to `SIMILAR_MIN_SCORE`: every pair
/// whose cosine (read from the stored vectors, since edges keep no score)
/// is below it is deleted in both directions, unless it is a memory's floor:
/// its closest pair at or above `SIMILAR_FLOOR_SCORE` when the memory would
/// otherwise keep fewer than `SIMILAR_MIN_EDGES`. A memory left with none
/// is then linked to its closest neighbour above the floor, if any.
/// Idempotent: a second run keeps the same pairs and adds nothing, and a
/// run cut short is completed by the next.
pub fn prune_similar(store: &dyn GraphStore, repo: &str) -> Result<PruneSummary> {
    use std::collections::{BTreeMap, HashMap, HashSet};
    let mut summary = PruneSummary::default();
    let mut memories = Vec::new();
    for &kind in MEMORY_KINDS {
        memories.extend(store.nodes_by_kind(repo, kind)?.into_iter().map(|n| n.id));
    }
    summary.memories = memories.len();
    summary.unlinked_before = count_unlinked(store, repo, &memories)?;

    // Every undirected SimilarTo pair, with its cosine (None: a vector is missing).
    let mut pairs: BTreeMap<(i64, i64), Option<f32>> = BTreeMap::new();
    for &id in &memories {
        let targets: Vec<i64> = store.edges_from(repo, id)?.into_iter().filter(|e| e.relation == EdgeRelation::SimilarTo).map(|e| e.dst_id).collect();
        if targets.is_empty() {
            continue;
        }
        let vectors = store.get_embeddings(repo, &[&[id][..], &targets].concat())?;
        for t in targets {
            let score = match (vectors.get(&id), vectors.get(&t)) {
                (Some(a), Some(b)) => Some(cosine(a, b)),
                _ => None,
            };
            pairs.entry((id.min(t), id.max(t))).or_insert(score);
        }
    }
    let strong = |s: &Option<f32>| s.is_some_and(|s| s >= SIMILAR_MIN_SCORE);
    let mut keep: HashSet<(i64, i64)> = pairs.iter().filter(|(_, s)| strong(s)).map(|(p, _)| *p).collect();
    let mut by_node: HashMap<i64, Vec<((i64, i64), f32)>> = HashMap::new();
    for (&pair, score) in &pairs {
        if let Some(s) = *score {
            by_node.entry(pair.0).or_default().push((pair, s));
            by_node.entry(pair.1).or_default().push((pair, s));
        }
    }
    for &id in &memories {
        let Some(mine) = by_node.get_mut(&id) else { continue };
        let kept = mine.iter().filter(|(p, _)| keep.contains(p)).count();
        if kept >= SIMILAR_MIN_EDGES {
            continue;
        }
        mine.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let floor: Vec<(i64, i64)> = mine.iter().filter(|(p, s)| *s >= SIMILAR_FLOOR_SCORE && !keep.contains(p)).take(SIMILAR_MIN_EDGES - kept).map(|(p, _)| *p).collect();
        summary.floor_pairs += floor.len();
        keep.extend(floor);
    }
    for &(a, b) in pairs.keys().filter(|p| !keep.contains(p)) {
        for (src, dst) in [(a, b), (b, a)] {
            let ids: Vec<i64> = store.edges_from(repo, src)?.into_iter().filter(|e| e.relation == EdgeRelation::SimilarTo && e.dst_id == dst).map(|e| e.id).collect();
            store.delete_edges_batch(repo, &ids)?;
        }
        summary.pairs_pruned += 1;
    }

    // A memory with no SimilarTo edge left: its closest neighbour above the floor.
    for &id in &memories {
        if similar_degree(store, repo, id)? >= SIMILAR_MIN_EDGES {
            continue;
        }
        let Some(embedding) = store.get_embedding(repo, id)? else { continue };
        if let Some((neighbour, _)) = similar_memories(store, repo, &embedding, Some(id), 1)?.into_iter().find(|(_, s)| *s >= SIMILAR_FLOOR_SCORE) {
            link_one(store, repo, id, neighbour.id, false)?;
            link_one(store, repo, neighbour.id, id, false)?;
            summary.floor_pairs += 1;
        }
    }
    summary.unlinked_after = count_unlinked(store, repo, &memories)?;
    Ok(summary)
}

/// Memories with no edge of any kind, in or out.
fn count_unlinked(store: &dyn GraphStore, repo: &str, memories: &[i64]) -> Result<usize> {
    let mut n = 0;
    for &id in memories {
        if store.edges_from(repo, id)?.is_empty() && store.edges_to(repo, id)?.is_empty() {
            n += 1;
        }
    }
    Ok(n)
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

    /// A unit vector at `degrees` in the plane of dimensions `x` and `x + 1`:
    /// two of them have cosine `cos(difference)`.
    fn angled(x: usize, degrees: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 384];
        (v[x], v[x + 1]) = (degrees.to_radians().cos(), degrees.to_radians().sin());
        v
    }

    fn pair(store: &SqliteGraphStore, a: i64, b: i64) {
        store.add_edge("r", a, b, EdgeRelation::SimilarTo).unwrap();
        store.add_edge("r", b, a, EdgeRelation::SimilarTo).unwrap();
    }

    #[test]
    fn a_prune_keeps_strong_pairs_and_one_floor_pair_per_memory_relinks_the_unlinked_and_is_idempotent() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = memory(&store, NodeKind::Note, "a", &angled(0, 0.0));
        let b = memory(&store, NodeKind::Note, "b", &angled(0, 25.0)); // a-b 0.906
        let c = memory(&store, NodeKind::Note, "c", &angled(0, -40.0)); // a-c 0.766
        let d = memory(&store, NodeKind::Note, "d", &angled(0, -77.0)); // c-d 0.799
        let e = memory(&store, NodeKind::Gotcha, "e", &angled(10, 0.0));
        let f = memory(&store, NodeKind::Gotcha, "f", &angled(10, 53.0)); // e-f 0.602
        let g = memory(&store, NodeKind::Decision, "g", &angled(0, 63.0)); // g-b 0.788, no edge yet
        for (x, y) in [(a, b), (a, c), (c, d), (e, f)] {
            pair(&store, x, y);
        }

        let first = prune_similar(&store, "r").unwrap();
        assert_eq!((first.memories, first.unlinked_before, first.unlinked_after), (7, 1, 2), "{first:?}");
        assert_eq!(first.pairs_pruned, 2, "a-c is under the threshold and not c's best; e-f is under the floor: {first:?}");
        assert_eq!(first.floor_pairs, 2, "c-d kept as c's floor, g linked to b: {first:?}");
        assert_eq!(similar_to(&store, a), vec![b]);
        assert_eq!(similar_to(&store, c), vec![d]);
        assert_eq!(similar_to(&store, g), vec![b]);
        assert!(similar_to(&store, e).is_empty() && similar_to(&store, f).is_empty(), "nothing above the floor for them");

        let edges = |store: &SqliteGraphStore| [a, b, c, d, e, f, g].map(|id| similar_to(store, id));
        let before = edges(&store);
        let second = prune_similar(&store, "r").unwrap();
        assert_eq!((second.pairs_pruned, second.unlinked_after), (0, 2), "{second:?}");
        assert_eq!(edges(&store), before, "a second run changes nothing");
    }

    #[test]
    fn a_write_with_nothing_above_the_threshold_keeps_its_closest_neighbour_above_the_floor() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let near = memory(&store, NodeKind::Note, "near", &angled(0, 38.0)); // 0.788
        let too_far = memory(&store, NodeKind::Note, "too-far", &angled(0, -60.0)); // 0.5
        let new = memory(&store, NodeKind::Note, "new", &angled(0, 0.0));
        link_similar(&store, "r", new).unwrap();
        assert_eq!(similar_to(&store, new), vec![near]);
        assert!(!similar_to(&store, new).contains(&too_far));
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
