//! Experience replay buffer construction (Initiative 5, CLS-inspired
//! retrieval plan) -- positive pairs drawn from the graph's own
//! plasticity-shaped `Affects`/`References` edges, hard negatives mined
//! from nearby-but-unconnected nodes, and an anchor set from prior runs
//! interleaved in to guard against the projection head overfitting to only
//! the latest session. This is the literal 2026 continual-learning
//! technique (replay + a small adapter), not a metaphor.

use std::collections::HashSet;

use agentops_graph::{effective_edge_weight, EdgeRelation, GraphStore};
use anyhow::Result;

/// Below this many candidate positive pairs, consolidation isn't run at
/// all -- too little signal to be more than overfitting to a handful of
/// edges. Deliberately a small, tunable constant, not derived from
/// anything.
pub const MIN_REPLAY_PAIRS: usize = 10;
/// Hard ceiling on how many positive pairs one consolidation pass
/// processes -- each one costs a `search_similar` call for hard-negative
/// mining, so this bounds worst-case cost on a large, unscoped replay
/// buffer the same way this codebase bounds every other potentially
/// expensive graph operation.
pub const MAX_REPLAY_PAIRS: usize = 500;
/// How many previously-seen pairs are kept as the anchor set and
/// interleaved into every future run.
pub const ANCHOR_SIZE: usize = 200;
/// How many nearest neighbors `search_similar` considers per anchor when
/// mining a hard negative.
const NEGATIVE_SEARCH_K: usize = 10;

/// One graph-derived positive pair with its plasticity weight -- also the
/// shape the anchor set (Initiative 5's interleaving mechanism) is
/// persisted in, so `ProjectionStore` can round-trip it as-is.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ReplayPair {
    pub anchor_id: i64,
    pub positive_id: i64,
    pub weight: f64,
}

/// One fully-resolved training example: a positive pair plus a mined hard
/// negative, ready to be embedded and trained on.
#[derive(Debug, Clone, Copy)]
pub struct ReplayExample {
    pub anchor_id: i64,
    pub positive_id: i64,
    pub negative_id: i64,
    pub weight: f64,
}

/// Collects candidate positive pairs from `Affects`/`References` edges,
/// weighted by `effective_edge_weight` -- the existing plasticity/decay
/// substrate, directly implementing salience-weighted replay (a
/// heavily-reinforced, recently-confirmed edge contributes more to
/// training than a stale, barely-touched one) via loss weighting rather
/// than sampling frequency, which is deterministic and doesn't need an RNG
/// for this step. `scope`, if given, only keeps edges touching at least
/// one of those node ids (the session-scoped replay Initiative 5's
/// `end_session` trigger uses); `None` means "every plasticity-bearing
/// edge in the repo."
pub fn collect_positive_pairs(store: &dyn GraphStore, repo: &str, scope: Option<&[i64]>) -> Result<Vec<ReplayPair>> {
    let mut pairs = Vec::new();
    for edge in store.all_edges(repo)? {
        if !matches!(edge.relation, EdgeRelation::Affects | EdgeRelation::References) {
            continue;
        }
        if let Some(scope) = scope {
            if !scope.contains(&edge.src_id) && !scope.contains(&edge.dst_id) {
                continue;
            }
        }
        pairs.push(ReplayPair { anchor_id: edge.src_id, positive_id: edge.dst_id, weight: effective_edge_weight(&edge) });
    }
    Ok(pairs)
}

/// Merges freshly-collected `session_pairs` with the persisted `anchor`
/// set from prior runs, deduplicated by `(anchor_id, positive_id)` (a
/// session pair wins over an anchor duplicate, since its weight is more
/// current), then caps the combined pool at `MAX_REPLAY_PAIRS`, keeping
/// the highest-weight pairs -- so a huge unscoped replay doesn't blow past
/// the mining-cost ceiling `MAX_REPLAY_PAIRS` exists for. Also returns the
/// next anchor set to persist: the top `ANCHOR_SIZE` pairs from this
/// merged pool, so a future run keeps replaying a representative sample
/// even if today's session data ages out of relevance.
pub fn merge_with_anchor(session_pairs: Vec<ReplayPair>, anchor: &[ReplayPair]) -> (Vec<ReplayPair>, Vec<ReplayPair>) {
    let mut by_key: std::collections::HashMap<(i64, i64), ReplayPair> = std::collections::HashMap::new();
    for pair in anchor.iter().chain(session_pairs.iter()) {
        // Iterating anchor first then session means a session pair with
        // the same key overwrites the anchor's (possibly stale) weight.
        by_key.insert((pair.anchor_id, pair.positive_id), *pair);
    }
    let mut merged: Vec<ReplayPair> = by_key.into_values().collect();
    merged.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    merged.truncate(MAX_REPLAY_PAIRS);

    let next_anchor: Vec<ReplayPair> = merged.iter().take(ANCHOR_SIZE).copied().collect();
    (merged, next_anchor)
}

/// Resolves each positive pair into a full training example by mining a
/// hard negative -- the nearest embedding-space neighbor to `anchor_id`
/// that isn't `anchor_id` itself, isn't the actual positive, and isn't
/// connected to `anchor_id` by any plasticity-bearing edge in `pairs`
/// (checked both directions). Pairs whose anchor/positive was never
/// embedded, or for which no valid negative can be mined, are silently
/// dropped -- there's nothing to train on for them yet, not an error.
pub fn resolve_examples(store: &dyn GraphStore, repo: &str, pairs: &[ReplayPair]) -> Result<Vec<ReplayExample>> {
    let adjacency: HashSet<(i64, i64)> = pairs.iter().flat_map(|p| [(p.anchor_id, p.positive_id), (p.positive_id, p.anchor_id)]).collect();

    let mut examples = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let Some(anchor_embedding) = store.get_embedding(repo, pair.anchor_id)? else { continue };
        if store.get_embedding(repo, pair.positive_id)?.is_none() {
            continue;
        }

        let negative_id = store
            .search_similar(repo, &anchor_embedding, NEGATIVE_SEARCH_K, None)?
            .into_iter()
            .map(|(node, _)| node.id)
            .find(|&id| id != pair.anchor_id && id != pair.positive_id && !adjacency.contains(&(pair.anchor_id, id)));

        if let Some(negative_id) = negative_id {
            examples.push(ReplayExample { anchor_id: pair.anchor_id, positive_id: pair.positive_id, negative_id, weight: pair.weight });
        }
    }
    Ok(examples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{upsert_node, NewNode, NodeKind, SqliteGraphStore};

    fn symbol(store: &dyn GraphStore, repo: &str, name: &str) -> i64 {
        upsert_node(store, NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(format!("{name}.rs")), name: Some(name.into()), container: None, start_line: Some(1), end_line: Some(2), content: Some(name.into()) }).unwrap()
    }

    fn unit_vec(dim: usize, dominant: usize) -> Vec<f32> {
        let mut v = vec![0.01f32; dim];
        v[dominant] = 1.0;
        v
    }

    #[test]
    fn collect_positive_pairs_only_takes_plasticity_bearing_relations() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = symbol(&store, "demo", "a");
        let b = symbol(&store, "demo", "b");
        let c = symbol(&store, "demo", "c");
        store.add_edge("demo", a, b, EdgeRelation::References).unwrap();
        store.add_edge("demo", a, c, EdgeRelation::DependsOn).unwrap();

        let pairs = collect_positive_pairs(&store, "demo", None).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!((pairs[0].anchor_id, pairs[0].positive_id), (a, b));
    }

    #[test]
    fn collect_positive_pairs_respects_scope() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = symbol(&store, "demo", "a");
        let b = symbol(&store, "demo", "b");
        let c = symbol(&store, "demo", "c");
        let d = symbol(&store, "demo", "d");
        store.add_edge("demo", a, b, EdgeRelation::References).unwrap();
        store.add_edge("demo", c, d, EdgeRelation::References).unwrap();

        let pairs = collect_positive_pairs(&store, "demo", Some(&[a])).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!((pairs[0].anchor_id, pairs[0].positive_id), (a, b));
    }

    #[test]
    fn merge_with_anchor_deduplicates_and_prefers_the_session_weight() {
        let session = vec![ReplayPair { anchor_id: 1, positive_id: 2, weight: 3.0 }];
        let anchor = vec![ReplayPair { anchor_id: 1, positive_id: 2, weight: 1.0 }, ReplayPair { anchor_id: 5, positive_id: 6, weight: 2.0 }];

        let (merged, _) = merge_with_anchor(session, &anchor);
        assert_eq!(merged.len(), 2);
        let dup = merged.iter().find(|p| p.anchor_id == 1 && p.positive_id == 2).unwrap();
        assert_eq!(dup.weight, 3.0, "the fresher session weight must win over the stale anchor one");
    }

    #[test]
    fn merge_with_anchor_caps_at_max_replay_pairs_keeping_highest_weight() {
        let session: Vec<ReplayPair> = (0..(MAX_REPLAY_PAIRS + 50)).map(|i| ReplayPair { anchor_id: i as i64, positive_id: (i as i64) + 100_000, weight: i as f64 }).collect();
        let (merged, next_anchor) = merge_with_anchor(session, &[]);
        assert_eq!(merged.len(), MAX_REPLAY_PAIRS);
        assert!(merged.iter().all(|p| p.weight >= 49.0), "the lowest-weight pairs must be the ones dropped");
        assert_eq!(next_anchor.len(), ANCHOR_SIZE);
    }

    #[test]
    fn resolve_examples_mines_a_negative_that_is_not_positively_connected() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = symbol(&store, "demo", "a");
        let b = symbol(&store, "demo", "b");
        let unconnected = symbol(&store, "demo", "unconnected");
        store.add_edge("demo", a, b, EdgeRelation::References).unwrap();
        store.set_embedding("demo", a, &unit_vec(384, 0)).unwrap();
        store.set_embedding("demo", b, &unit_vec(384, 0)).unwrap();
        store.set_embedding("demo", unconnected, &unit_vec(384, 0)).unwrap();

        let pairs = vec![ReplayPair { anchor_id: a, positive_id: b, weight: 1.0 }];
        let examples = resolve_examples(&store, "demo", &pairs).unwrap();
        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].negative_id, unconnected);
    }

    #[test]
    fn resolve_examples_drops_a_pair_whose_anchor_was_never_embedded() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = symbol(&store, "demo", "a");
        let b = symbol(&store, "demo", "b");
        store.add_edge("demo", a, b, EdgeRelation::References).unwrap();
        // Deliberately no set_embedding call for either node.

        let pairs = vec![ReplayPair { anchor_id: a, positive_id: b, weight: 1.0 }];
        let examples = resolve_examples(&store, "demo", &pairs).unwrap();
        assert!(examples.is_empty());
    }
}
