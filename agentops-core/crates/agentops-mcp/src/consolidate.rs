//! Wires Initiative 5 (embedding consolidation, CLS-inspired retrieval
//! plan) into `agentops-mcp`: this is the one place besides
//! `agentops-embeddings-train` itself that touches `candle` -- neither
//! `agentops-retrieval` (its `EmbeddingProjector` trait exists precisely
//! so it doesn't have to) nor any of the other tool-handling code in this
//! crate needs to know a training framework exists.

use agentops_embeddings_train::{ConsolidationReport, ProjectionHead, ProjectionStore};
use agentops_graph::GraphStore;
use agentops_retrieval::EmbeddingProjector;
use anyhow::Result;
use candle_core::Device;

/// Runs Initiative 5's consolidation pass for `repo`. `scope`, if given,
/// restricts the replay buffer to edges touching those node ids; `None`
/// uses every `Affects`/`References` edge in the repo.
///
/// **Deliberately `scope: None` from `tool_end_session`, not narrowed to
/// the calling session's own touched nodes** -- the plan this shipped
/// against assumed `session_events` rows could be used to recover which
/// node ids a session actually touched, but `SessionEvent.description` is
/// free-form human-readable text ("explained symbol 123", "added note:
/// ..."), not structured data; reliably parsing node ids back out of it
/// would be fragile regex-guessing, not a real fix. `end_session` still
/// gates on the session having recorded *some* activity (a real, if
/// coarser, signal that consolidation is warranted) — see
/// `tool_end_session`'s own call site. Narrowing this properly would mean
/// extending `SessionEvent` with a structured `node_ids` column, which is
/// real, separate schema work, not something to bolt on silently here.
pub fn run_embedding_consolidation(store: &dyn GraphStore, repo: &str) -> Result<ConsolidationReport> {
    agentops_embeddings_train::consolidate(store, repo, None)
}

/// Adapts a loaded `ProjectionHead` to `agentops_retrieval::EmbeddingProjector`
/// -- the seam that trait exists for.
pub struct LoadedProjector {
    head: ProjectionHead,
    device: Device,
}

impl EmbeddingProjector for LoadedProjector {
    fn project(&self, embedding: &[f32]) -> Vec<f32> {
        // A projection failure (should be unreachable for a well-formed,
        // correctly-dimensioned embedding) degrades to the raw embedding
        // rather than panicking a search request -- the same
        // fail-open-to-baseline posture `Node.last_touched_at: None`
        // already established for Initiative 3.
        self.head.apply_one(&self.device, embedding).unwrap_or_else(|_| embedding.to_vec())
    }
}

/// Loads `repo`'s currently-active projection, if any consolidation run
/// has ever promoted one. `None` for a never-consolidated repo -- callers
/// pass this straight through to `search_hybrid`'s `projector` parameter,
/// where `None` already means "rank on raw embeddings, unchanged."
pub fn load_active_projector(repo: &str) -> Option<LoadedProjector> {
    let device = Device::Cpu;
    let store = ProjectionStore::open(repo).ok()?;
    let head = store.load_active(&device).ok().flatten()?;
    Some(LoadedProjector { head, device })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{upsert_node, EdgeRelation, NewNode, NodeKind, SqliteGraphStore};

    fn symbol(store: &dyn GraphStore, repo: &str, name: &str) -> i64 {
        upsert_node(store, NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(format!("{name}.rs")), name: Some(name.into()), container: None, start_line: Some(1), end_line: Some(2), content: Some(name.into()) }).unwrap()
    }

    #[test]
    fn load_active_projector_is_none_for_a_repo_that_was_never_consolidated() {
        assert!(load_active_projector("a-repo-that-has-definitely-never-been-consolidated-xyz").is_none());
    }

    #[test]
    fn run_embedding_consolidation_skips_gracefully_with_too_few_pairs() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = symbol(&store, "demo-consolidate-mcp", "a");
        let b = symbol(&store, "demo-consolidate-mcp", "b");
        store.add_edge("demo-consolidate-mcp", a, b, EdgeRelation::References).unwrap();

        let report = run_embedding_consolidation(&store, "demo-consolidate-mcp").unwrap();
        assert!(!report.attempted);
    }
}
