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
    /// Every synced session's usage, main agent + subagents.
    pub tokens: UsageTotals,
    /// The subagent share of `tokens` (rows whose session_id ends in
    /// `agentops_graph::SUBAGENT_SESSION_SUFFIX`) — already included above.
    pub subagent_tokens: UsageTotals,
    /// Most recent sessions first, at most `RECENT_SESSIONS` — the
    /// per-improvement view: what one working session cost.
    pub sessions: Vec<SessionBreakdown>,
    /// Count of `session_events` rows tagged `event_kind: "hit"` — a real,
    /// exact count, unlike everything below it.
    pub hit_count: i64,
    /// Estimated only — see this module's doc comment. Never rename to
    /// imply precision (e.g. `tokens_saved`) in any consumer of this type.
    pub estimated_tokens_saved: i64,
    pub estimated_cost_saved_usd: f64,
    /// AgentOps' *own* LLM spend (`llm_usage`: explain_symbol, note
    /// classification, docgen, ...), one entry per (operation, provider,
    /// model) — kept apart from `tokens` above, which is coding-agent
    /// session usage, so neither skews the other or the savings estimate.
    pub llm_spend: Vec<LlmSpend>,
}

/// How many recent sessions `UsageSummary::sessions` lists.
const RECENT_SESSIONS: usize = 10;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SessionBreakdown {
    pub session_id: String,
    pub started_at: String,
    pub ended_at: String,
    /// input + output + cache read + cache write, main agent only.
    pub main_tokens: i64,
    pub subagent_tokens: i64,
    /// Largest single request's context in the main agent — how big the
    /// conversation got. Not part of the totals and never summable.
    pub peak_context_tokens: i64,
    pub cost_usd: f64,
}

fn total_of(row: &agentops_graph::SessionUsage) -> i64 {
    row.input_tokens + row.output_tokens + row.cache_read_tokens + row.cache_write_tokens
}

/// Folds `(session, model)` rows — and each session's `#subagents` rows —
/// into one breakdown per parent session, newest first.
fn session_breakdowns(rows: &[agentops_graph::SessionUsage]) -> Vec<SessionBreakdown> {
    let mut out: Vec<SessionBreakdown> = Vec::new();
    for row in rows {
        let (parent, is_subagent) = match row.session_id.strip_suffix(agentops_graph::SUBAGENT_SESSION_SUFFIX) {
            Some(parent) => (parent, true),
            None => (row.session_id.as_str(), false),
        };
        let idx = match out.iter().position(|b| b.session_id == parent) {
            Some(i) => i,
            None => {
                out.push(SessionBreakdown {
                    session_id: parent.to_string(),
                    started_at: row.session_started_at.clone(),
                    ended_at: row.session_ended_at.clone(),
                    main_tokens: 0,
                    subagent_tokens: 0,
                    peak_context_tokens: 0,
                    cost_usd: 0.0,
                });
                out.len() - 1
            }
        };
        let b = &mut out[idx];
        if is_subagent {
            b.subagent_tokens += total_of(row);
        } else {
            b.main_tokens += total_of(row);
            b.peak_context_tokens = b.peak_context_tokens.max(row.peak_context_tokens);
        }
        b.cost_usd += row.cost_estimate_usd;
        if row.session_started_at < b.started_at {
            b.started_at = row.session_started_at.clone();
        }
        if row.session_ended_at > b.ended_at {
            b.ended_at = row.session_ended_at.clone();
        }
    }
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    out.truncate(RECENT_SESSIONS);
    out
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LlmSpend {
    pub operation: String,
    pub provider: String,
    pub model: String,
    pub calls: i64,
    pub failures: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// `None` when *no* call in this group has a known rate.
    pub cost_estimate_usd: Option<f64>,
    /// True when at least one call in this group has no known rate, so
    /// `cost_estimate_usd` (if any) undercounts — a consumer must show this
    /// as partial, never as a complete dollar figure.
    pub cost_partial: bool,
}

/// Groups `llm_usage` rows by (operation, provider, model), in first-seen
/// order.
fn llm_spend(rows: &[agentops_graph::LlmUsage]) -> Vec<LlmSpend> {
    let mut out: Vec<LlmSpend> = Vec::new();
    for row in rows {
        let idx = match out.iter().position(|g| g.operation == row.operation && g.provider == row.provider && g.model == row.model) {
            Some(i) => i,
            None => {
                out.push(LlmSpend {
                    operation: row.operation.clone(),
                    provider: row.provider.clone(),
                    model: row.model.clone(),
                    calls: 0,
                    failures: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cost_estimate_usd: None,
                    cost_partial: false,
                });
                out.len() - 1
            }
        };
        let group = &mut out[idx];
        group.calls += 1;
        group.failures += i64::from(!row.success);
        group.input_tokens += row.input_tokens;
        group.output_tokens += row.output_tokens;
        match row.cost_estimate_usd {
            Some(cost) => group.cost_estimate_usd = Some(group.cost_estimate_usd.unwrap_or(0.0) + cost),
            None => group.cost_partial = true,
        }
    }
    out
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
    let mut subagent_tokens = UsageTotals::default();
    for row in &usage_rows {
        let is_subagent = row.session_id.ends_with(agentops_graph::SUBAGENT_SESSION_SUFFIX);
        for totals in [Some(&mut tokens), is_subagent.then_some(&mut subagent_tokens)].into_iter().flatten() {
            totals.input_tokens += row.input_tokens;
            totals.output_tokens += row.output_tokens;
            totals.cache_read_tokens += row.cache_read_tokens;
            totals.cache_write_tokens += row.cache_write_tokens;
            totals.cost_usd += row.cost_estimate_usd;
        }
    }
    let sessions = session_breakdowns(&usage_rows);

    let hits = store.session_events_for_repo(repo, Some("hit"))?;
    let hit_count = hits.len() as i64;

    let estimated_tokens_saved = hit_count * AVG_TOKENS_PER_RESEARCH_TURN;
    let estimated_cost_saved_usd = estimated_tokens_saved as f64 * effective_cost_per_token(&tokens);

    let llm_spend = llm_spend(&store.llm_usage_for_repo(repo)?);

    Ok(UsageSummary { repo: repo.to_string(), tokens, subagent_tokens, sessions, hit_count, estimated_tokens_saved, estimated_cost_saved_usd, llm_spend })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{NewLlmUsage, NewSessionUsage, SqliteGraphStore};

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
            peak_context_tokens: 0,
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
        assert_eq!(
            summary,
            UsageSummary {
                repo: "untouched-repo".into(),
                tokens: UsageTotals::default(),
                subagent_tokens: UsageTotals::default(),
                sessions: vec![],
                hit_count: 0,
                estimated_tokens_saved: 0,
                estimated_cost_saved_usd: 0.0,
                llm_spend: vec![],
            }
        );
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

    #[test]
    fn sessions_fold_subagent_rows_into_their_parent_and_list_newest_first() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let mut older = usage_row("repo-a", "sess-old", 100, 10, 0.5);
        older.session_started_at = "2026-10-01T00:00:00Z".into();
        store.upsert_session_usage(older).unwrap();
        let mut main = usage_row("repo-a", "sess-new", 1000, 100, 2.0);
        main.peak_context_tokens = 390_000;
        main.session_started_at = "2026-10-09T00:00:00Z".into();
        store.upsert_session_usage(main).unwrap();
        let mut sub = usage_row("repo-a", &format!("sess-new{}", agentops_graph::SUBAGENT_SESSION_SUFFIX), 300, 30, 0.25);
        sub.session_started_at = "2026-10-09T01:00:00Z".into();
        store.upsert_session_usage(sub).unwrap();

        let summary = usage_summary(&store, "repo-a").unwrap();
        assert_eq!(summary.tokens.input_tokens, 1400, "the all-in total includes subagents");
        assert_eq!(summary.subagent_tokens.input_tokens, 300);
        assert_eq!(summary.sessions.len(), 2);
        let newest = &summary.sessions[0];
        assert_eq!(newest.session_id, "sess-new");
        assert_eq!((newest.main_tokens, newest.subagent_tokens, newest.peak_context_tokens), (1100, 330, 390_000));
        assert_eq!(newest.cost_usd, 2.25);
        assert_eq!(summary.sessions[1].session_id, "sess-old");
    }

    fn llm_row(repo: &str, operation: &str, provider: &str, model: &str, cost: Option<f64>, success: bool) -> NewLlmUsage {
        NewLlmUsage {
            repo: repo.into(),
            operation: operation.into(),
            provider: provider.into(),
            model: model.into(),
            input_tokens: 100,
            output_tokens: 10,
            cost_estimate_usd: cost,
            latency_ms: 50,
            success,
            error_kind: (!success).then(|| "rate_limited".to_string()),
        }
    }

    #[test]
    fn llm_spend_groups_by_operation_provider_and_model_and_counts_failures() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.record_llm_usage(llm_row("repo-a", "explain_symbol", "anthropic", "claude-sonnet-5", Some(0.5), true)).unwrap();
        store.record_llm_usage(llm_row("repo-a", "explain_symbol", "anthropic", "claude-sonnet-5", Some(0.25), false)).unwrap();
        store.record_llm_usage(llm_row("repo-a", "classify_note", "anthropic", "claude-haiku-4-5", Some(0.01), true)).unwrap();

        let spend = usage_summary(&store, "repo-a").unwrap().llm_spend;
        assert_eq!(spend.len(), 2);
        assert_eq!((spend[0].operation.as_str(), spend[0].calls, spend[0].failures, spend[0].input_tokens), ("explain_symbol", 2, 1, 200));
        assert_eq!(spend[0].cost_estimate_usd, Some(0.75));
        assert!(!spend[0].cost_partial);
    }

    #[test]
    fn a_group_with_any_unpriced_call_is_marked_partial_never_silently_low() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.record_llm_usage(llm_row("repo-a", "librarian_classify", "groq", "llama", None, true)).unwrap();
        let spend = usage_summary(&store, "repo-a").unwrap().llm_spend;
        assert_eq!(spend[0].cost_estimate_usd, None);
        assert!(spend[0].cost_partial);

        store.record_llm_usage(llm_row("repo-b", "explain_symbol", "anthropic", "claude-mystery", None, true)).unwrap();
        store.record_llm_usage(llm_row("repo-b", "explain_symbol", "anthropic", "claude-mystery", Some(1.0), true)).unwrap();
        let mixed = usage_summary(&store, "repo-b").unwrap().llm_spend;
        assert_eq!(mixed[0].cost_estimate_usd, Some(1.0));
        assert!(mixed[0].cost_partial, "one unpriced call must flag the whole group as partial");
    }

    #[test]
    fn llm_spend_never_leaks_another_repos_rows() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.record_llm_usage(llm_row("repo-b", "explain_symbol", "anthropic", "claude-sonnet-5", Some(9.0), true)).unwrap();
        assert!(usage_summary(&store, "repo-a").unwrap().llm_spend.is_empty());
    }
}
