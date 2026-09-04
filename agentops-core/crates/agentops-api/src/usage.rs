//! Usage / knowledge-reuse dashboard aggregation (Module 8, CodeBurn-
//! inspired) — a pure function over an already-open `GraphStore`, composed
//! identically by `agentops-api`'s own handler (not yet wired — single-
//! operator mode has no per-connection dashboard route today) and
//! `agentops-heavy-api::dashboard`'s tenant-scoped one, matching this
//! crate's existing `repos`/`search`/`subgraph` convention (`pub` free
//! functions, no logic duplicated per caller).
//!
//! **Why this is an aggregate, not a per-event join**: `session_usage`
//! (real Claude Code session UUIDs, from `agentops-cli usage sync` parsing
//! local JSONL) and `session_events` (freeform, LLM-chosen `session_id`
//! strings passed to MCP tool calls) cannot be assumed to share the same
//! identifier space — see `agentops-graph::SessionUsage`'s own doc comment.
//! Rather than attempt a per-event time-window overlap join (a real
//! future refinement, not built here), this v1 aggregates each side
//! independently at the repo level and prices the estimate using the
//! repo's own overall usage mix — simpler, and no less honest, since the
//! result is presented as an *estimate* either way.

use agentops_graph::GraphStore;
use serde::Serialize;

/// Rough placeholder, not a measured counterfactual — there is no way to
/// observe how many tokens a session *would* have spent re-deriving
/// knowledge it instead got from a `list_gotchas`/`get_symbol`/
/// `related_context`/`semantic_search` "hit". Documented here, not buried
/// in a magic number, so a future revision has one place to reconsider it.
const AVG_TOKENS_PER_RESEARCH_TURN: i64 = 2000;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct UsageSummary {
    pub repo: String,
    pub tokens: UsageTotals,
    /// Count of `session_events` rows tagged `event_kind: "hit"` — a real,
    /// exact count, unlike everything below it.
    pub hit_count: i64,
    /// Estimated only — see this module's doc comment. Never rename to
    /// imply precision (e.g. `tokens_saved`) in any consumer of this type.
    pub estimated_tokens_saved: i64,
    pub estimated_cost_saved_usd: f64,
}

/// Effective $/token rate implied by `totals`' own recorded mix — used to
/// price the knowledge-reuse estimate consistently with what this repo's
/// sessions have actually been costing, rather than a second hardcoded
/// rate table diverging from `agentops-cli`'s.
fn effective_cost_per_token(totals: &UsageTotals) -> f64 {
    let total_tokens = totals.input_tokens + totals.output_tokens;
    if total_tokens == 0 {
        return 0.0;
    }
    totals.cost_usd / total_tokens as f64
}

pub fn usage_summary(store: &dyn GraphStore, repo: &str) -> anyhow::Result<UsageSummary> {
    let usage_rows = store.session_usage_for_repo(repo)?;
    let mut tokens = UsageTotals::default();
    for row in &usage_rows {
        tokens.input_tokens += row.input_tokens;
        tokens.output_tokens += row.output_tokens;
        tokens.cache_read_tokens += row.cache_read_tokens;
        tokens.cache_write_tokens += row.cache_write_tokens;
        tokens.cost_usd += row.cost_estimate_usd;
    }

    let hits = store.session_events_for_repo(repo, Some("hit"))?;
    let hit_count = hits.len() as i64;

    let estimated_tokens_saved = hit_count * AVG_TOKENS_PER_RESEARCH_TURN;
    let estimated_cost_saved_usd = estimated_tokens_saved as f64 * effective_cost_per_token(&tokens);

    Ok(UsageSummary { repo: repo.to_string(), tokens, hit_count, estimated_tokens_saved, estimated_cost_saved_usd })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{NewSessionUsage, SqliteGraphStore};

    fn usage_row(repo: &str, session_id: &str, input_tokens: i64, output_tokens: i64, cost_usd: f64) -> NewSessionUsage {
        NewSessionUsage {
            repo: repo.into(),
            session_id: session_id.into(),
            model: "claude-sonnet-5".into(),
            input_tokens,
            output_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_estimate_usd: cost_usd,
            session_started_at: "2026-09-03T00:00:00Z".into(),
            session_ended_at: "2026-09-03T01:00:00Z".into(),
        }
    }

    #[test]
    fn usage_summary_totals_usage_and_counts_hits_but_labels_savings_as_estimated() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.upsert_session_usage(usage_row("repo-a", "sess-1", 1_000_000, 500_000, 10.0)).unwrap();
        store.record_session_event("repo-a", "sess-x", "list_gotchas", "listed 2 gotcha(s)", None, "hit").unwrap();
        store.record_session_event("repo-a", "sess-y", "get_symbol", "looked up foo", Some(1), "hit").unwrap();
        // An "activity" row (a write tool) must never count as a hit.
        store.record_session_event("repo-a", "sess-z", "scan_repo", "scanned", None, "activity").unwrap();

        let summary = usage_summary(&store, "repo-a").unwrap();
        assert_eq!(summary.tokens.input_tokens, 1_000_000);
        assert_eq!(summary.tokens.output_tokens, 500_000);
        assert_eq!(summary.tokens.cost_usd, 10.0);
        assert_eq!(summary.hit_count, 2, "must count only 'hit' events, not 'activity'");
        assert_eq!(summary.estimated_tokens_saved, 2 * AVG_TOKENS_PER_RESEARCH_TURN);
        assert!(summary.estimated_cost_saved_usd > 0.0);
    }

    #[test]
    fn usage_summary_is_all_zero_for_a_repo_with_no_recorded_activity() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let summary = usage_summary(&store, "untouched-repo").unwrap();
        assert_eq!(summary, UsageSummary { repo: "untouched-repo".into(), tokens: UsageTotals::default(), hit_count: 0, estimated_tokens_saved: 0, estimated_cost_saved_usd: 0.0 });
    }

    #[test]
    fn usage_summary_never_leaks_another_repos_usage_or_hits() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.upsert_session_usage(usage_row("repo-b", "sess-1", 999, 999, 99.0)).unwrap();
        store.record_session_event("repo-b", "sess-1", "list_gotchas", "listed", None, "hit").unwrap();

        let summary = usage_summary(&store, "repo-a").unwrap();
        assert_eq!(summary.tokens, UsageTotals::default());
        assert_eq!(summary.hit_count, 0);
    }
}
