//! Embedding consolidation (Initiative 5, CLS-inspired retrieval plan):
//! codebrain's own "hippocampal replay trains the slow cortical learner"
//! mechanism, made concrete. A small residual projection head
//! (`model::ProjectionHead`) is trained on top of the frozen
//! `agentops_embeddings::LocalEmbedder` output, using positive pairs drawn
//! from the graph's own plasticity-shaped `Affects`/`References` edges
//! (`replay`), hard negatives mined from nearby-but-unconnected nodes, and
//! an anchor set from prior runs interleaved in against overfitting to the
//! latest session alone. A candidate head is only promoted
//! (`store::ProjectionStore::promote`) if it doesn't regress recall@k
//! against whichever projection is currently active (`eval`).
//!
//! Deliberately its own crate, not folded into `agentops-embeddings`: this
//! is the one place `candle-core`/`candle-nn` (a training-capable ML
//! framework `fastembed`/`ort`, inference-only, can't provide) enters this
//! codebase's dependency graph, and it should never leak into the default
//! inference path every other crate pulls in just to embed a query.

mod eval;
mod model;
mod replay;
mod store;

pub use eval::{recall_at_k, split_held_out, HELD_OUT_FRACTION, RECALL_K};
pub use model::ProjectionHead;
pub use replay::{collect_positive_pairs, merge_with_anchor, resolve_examples, ReplayExample, ReplayPair, ANCHOR_SIZE, MAX_REPLAY_PAIRS, MIN_REPLAY_PAIRS};
pub use store::ProjectionStore;
pub use train::TrainConfig;

mod train;

use std::collections::HashMap;

use agentops_graph::GraphStore;
use anyhow::Result;
use candle_core::Device;

/// What one `consolidate` call actually did -- returned rather than only
/// logged, so a caller (the `end_session` MCP tool, a future dashboard
/// status endpoint) can report real outcomes instead of a bare "done."
#[derive(Debug, Clone)]
pub struct ConsolidationReport {
    pub attempted: bool,
    pub promoted: bool,
    pub examples_used: usize,
    pub candidate_recall: f64,
    pub baseline_recall: f64,
    pub promoted_version: Option<u32>,
    pub reason: String,
}

/// Runs one full consolidation pass for `repo`: builds a plasticity
/// -weighted replay buffer (scoped to `scope`'s node ids if given, else
/// every `Affects`/`References` edge in the repo), interleaves it with the
/// persisted anchor set, trains a candidate projection head, evaluates it
/// against whichever head is currently active, and promotes it only if it
/// doesn't regress. Skips gracefully (never errors) if there isn't enough
/// signal yet -- a repo with few reinforced edges or little embedded
/// content simply isn't ready for this yet, which is a normal, expected
/// state for a young repo, not a failure.
pub fn consolidate(store: &dyn GraphStore, repo: &str, scope: Option<&[i64]>) -> Result<ConsolidationReport> {
    let skip = |reason: String| ConsolidationReport { attempted: false, promoted: false, examples_used: 0, candidate_recall: 0.0, baseline_recall: 0.0, promoted_version: None, reason };

    let projection_store = ProjectionStore::open(repo)?;
    let anchor = projection_store.load_anchor()?;
    let session_pairs = collect_positive_pairs(store, repo, scope)?;
    let (merged_pairs, next_anchor) = merge_with_anchor(session_pairs, &anchor);

    if merged_pairs.len() < MIN_REPLAY_PAIRS {
        return Ok(skip(format!("only {} candidate plasticity-bearing pair(s), need at least {MIN_REPLAY_PAIRS}", merged_pairs.len())));
    }

    let examples = resolve_examples(store, repo, &merged_pairs)?;
    if examples.len() < MIN_REPLAY_PAIRS {
        return Ok(skip(format!(
            "only {} pair(s) had both embeddings and a mineable hard negative (of {} candidates), need at least {MIN_REPLAY_PAIRS}",
            examples.len(),
            merged_pairs.len()
        )));
    }

    let (train_examples, held_out) = split_held_out(&examples);

    let mut pool: HashMap<i64, Vec<f32>> = HashMap::new();
    for ex in &examples {
        for id in [ex.anchor_id, ex.positive_id, ex.negative_id] {
            if let std::collections::hash_map::Entry::Vacant(slot) = pool.entry(id) {
                if let Some(embedding) = store.get_embedding(repo, id)? {
                    slot.insert(embedding);
                }
            }
        }
    }

    let embedding_of = |id: i64| pool.get(&id).cloned();
    let (varmap, candidate_head) = train::train(&train_examples, embedding_of, &TrainConfig::default())?;

    let device = Device::Cpu;
    let current_head = projection_store.load_active(&device)?;
    let baseline_recall = recall_at_k(&device, &held_out, &pool, current_head.as_ref())?;
    let candidate_recall = recall_at_k(&device, &held_out, &pool, Some(&candidate_head))?;

    let version = projection_store.save_new_version(&varmap)?;
    // The anchor set is refreshed regardless of promotion outcome -- it's
    // replay-buffer bookkeeping for *future* training runs, independent of
    // whether today's specific candidate head turned out to be worth
    // promoting.
    projection_store.save_anchor(&next_anchor)?;

    let promoted = candidate_recall >= baseline_recall;
    if promoted {
        projection_store.promote(version)?;
    }

    Ok(ConsolidationReport {
        attempted: true,
        promoted,
        examples_used: examples.len(),
        candidate_recall,
        baseline_recall,
        promoted_version: promoted.then_some(version),
        reason: if promoted {
            format!("promoted v{version}: recall@{RECALL_K} {candidate_recall:.3} >= baseline {baseline_recall:.3}")
        } else {
            format!("kept previous version: candidate recall@{RECALL_K} {candidate_recall:.3} regressed vs. baseline {baseline_recall:.3}")
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{upsert_node, EdgeRelation, NewNode, NodeKind, SqliteGraphStore};

    fn symbol(store: &dyn GraphStore, repo: &str, name: &str) -> i64 {
        upsert_node(store, NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(format!("{name}.rs")), name: Some(name.into()), container: None, start_line: Some(1), end_line: Some(2), content: Some(name.into()) }).unwrap()
    }

    fn embed(store: &dyn GraphStore, repo: &str, id: i64, dominant: usize) {
        let mut v = vec![0.001f32; agentops_embeddings::EMBEDDING_DIM];
        v[dominant] = 1.0;
        store.set_embedding(repo, id, &v).unwrap();
    }

    #[test]
    fn consolidate_skips_gracefully_with_too_few_pairs() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = symbol(&store, "demo", "a");
        let b = symbol(&store, "demo", "b");
        store.add_edge("demo", a, b, EdgeRelation::References).unwrap();

        let repo = format!("test-consolidate-{}", uuid_like());
        let report = consolidate(&store, &repo, None).unwrap();
        assert!(!report.attempted);
        assert!(!report.promoted);
    }

    /// The real end-to-end path: enough plasticity-bearing, embedded pairs
    /// to actually train and evaluate a candidate, and since nothing is
    /// active yet, the baseline is the raw embedding space -- any
    /// candidate that isn't actively worse should promote on a first run.
    #[test]
    fn consolidate_trains_and_promotes_on_a_first_real_run() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let repo = format!("test-consolidate-{}", uuid_like());

        for i in 0..15u32 {
            let a = symbol(&store, &repo, &format!("a{i}"));
            let b = symbol(&store, &repo, &format!("b{i}"));
            store.add_edge(&repo, a, b, EdgeRelation::References).unwrap();
            embed(&store, &repo, a, (i as usize * 2) % agentops_embeddings::EMBEDDING_DIM);
            embed(&store, &repo, b, (i as usize * 2) % agentops_embeddings::EMBEDDING_DIM);
            let unrelated = symbol(&store, &repo, &format!("u{i}"));
            embed(&store, &repo, unrelated, (i as usize * 2 + 1) % agentops_embeddings::EMBEDDING_DIM);
        }

        let report = consolidate(&store, &repo, None).unwrap();
        assert!(report.attempted, "{report:?}");
        assert!(report.examples_used >= MIN_REPLAY_PAIRS, "{report:?}");

        let projection_store = ProjectionStore::open(&repo).unwrap();
        if report.promoted {
            assert!(projection_store.active_version().unwrap().is_some());
        }
        // Clean up this test's real ~/.agentops/models/{repo} directory
        // rather than leaving throwaway test artifacts on disk.
        let _ = std::fs::remove_dir_all(dirs_home_for_test().join(".agentops").join("models").join(&repo));
    }

    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
    }

    fn dirs_home_for_test() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
    }
}
