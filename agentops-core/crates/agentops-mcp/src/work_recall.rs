//! Recall during work (milestone 3b): what the `PostToolUse` /
//! `PostToolUseFailure` hooks do with a work cue, kept apart from
//! recording so `agentops work-recall-eval --replay` runs the same code
//! against an in-memory event log.
//!
//! Per cue: resolve it (`agentops_retrieval::work_recall::resolve_cue`),
//! record its candidates as one `work_cue` event, and compute session
//! activation from the context's recent `work_cue` events (ADR 0053).
//!
//! **A context** is the main session, or one subagent (`agent_id`): each
//! has its own context window, so each has its own activation. The main
//! context starts over after a compaction (`context_reset`, recorded by
//! `SessionStart` with `source: compact`); a subagent's never does, since
//! Claude Code gives no signal for it.

use std::collections::HashMap;

use agentops_graph::SessionEvent;
use agentops_retrieval::work_recall::{activation, WorkCandidate, ACTIVATION_DECAY, ACTIVATION_WINDOW};

/// `event_kind` of one cue's candidates (description `id:score,...`, best
/// first). Recorded only when a cue reached something.
pub const WORK_CUE: &str = "work_cue";
/// `event_kind` recorded when the main context is compacted: activation
/// and the shown set count only events after the latest one.
pub const CONTEXT_RESET: &str = "context_reset";
/// Candidates one `work_cue` event keeps: enough for activation to see
/// what a cue reached beyond what it injects, small enough to parse on
/// every hook call.
pub const WORK_CUE_KEEP: usize = 10;

/// The events of one context, oldest first: those carrying `agent_id`
/// (`None` = the main session), and for the main session only those after
/// its latest `context_reset`.
pub fn context_events<'a>(events: &'a [SessionEvent], agent_id: Option<&str>) -> Vec<&'a SessionEvent> {
    let mine: Vec<&SessionEvent> = events.iter().filter(|e| e.agent_id.as_deref() == agent_id).collect();
    match mine.iter().rposition(|e| e.event_kind == CONTEXT_RESET) {
        Some(i) if agent_id.is_none() => mine[i + 1..].to_vec(),
        _ => mine,
    }
}

/// `id:score,...` for a `work_cue` event, best first, at most
/// `WORK_CUE_KEEP`.
pub fn format_cue(candidates: &[(i64, f64)]) -> String {
    candidates.iter().take(WORK_CUE_KEEP).map(|(id, score)| format!("{id}:{score:.3}")).collect::<Vec<_>>().join(",")
}

/// A `work_cue` event's `(id, score)` pairs; malformed entries are skipped.
pub fn parse_cue(description: &str) -> Vec<(i64, f64)> {
    description.split(',').filter_map(|pair| {
        let (id, score) = pair.trim().split_once(':')?;
        Some((id.parse().ok()?, score.parse().ok()?))
    }).collect()
}

/// `(id, score)` pairs of resolved candidates, for `format_cue`.
pub fn cue_scores(candidates: &[WorkCandidate]) -> Vec<(i64, f64)> {
    candidates.iter().map(|c| (c.pointer.node_id, c.score)).collect()
}

/// Session activation for a context whose events are `context` (see
/// `context_events`), plus `current`, the cue being handled now, which
/// isn't recorded yet.
pub fn context_activation(context: &[&SessionEvent], current: &[(i64, f64)]) -> HashMap<i64, f64> {
    let mut cues: Vec<Vec<(i64, f64)>> = context.iter().filter(|e| e.event_kind == WORK_CUE).map(|e| parse_cue(&e.description)).collect();
    if !current.is_empty() {
        cues.push(current.to_vec());
    }
    activation(&cues, ACTIVATION_WINDOW, ACTIVATION_DECAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, description: &str, agent: Option<&str>) -> SessionEvent {
        SessionEvent { id: 0, repo: "r".into(), session_id: "s".into(), tool_name: "t".into(), description: description.into(), node_id: None, event_kind: kind.into(), created_at: String::new(), agent_id: agent.map(String::from) }
    }

    #[test]
    fn a_cue_round_trips_through_its_event_description() {
        let cue = format_cue(&[(12, 0.8125), (40, 0.33)]);
        assert_eq!(cue, "12:0.812,40:0.330");
        assert_eq!(parse_cue(&cue), [(12, 0.812), (40, 0.33)]);
        assert_eq!(parse_cue("12:0.8,junk,7:x"), [(12, 0.8)]);
        assert_eq!(format_cue(&(0..20).map(|i| (i, 0.5)).collect::<Vec<_>>()).split(',').count(), WORK_CUE_KEEP);
    }

    #[test]
    fn each_context_has_its_own_activation_and_compaction_resets_only_the_main_one() {
        let events = vec![
            event(WORK_CUE, "1:1.0", None),
            event(WORK_CUE, "2:1.0", Some("agent-a")),
            event(CONTEXT_RESET, "compact", None),
            event(WORK_CUE, "3:1.0", None),
            event(CONTEXT_RESET, "compact", Some("agent-a")),
            event(WORK_CUE, "4:1.0", Some("agent-a")),
        ];
        let main = context_activation(&context_events(&events, None), &[]);
        assert_eq!(main.keys().copied().collect::<Vec<_>>(), [3], "main sees only its own cues after its compaction");
        let mut sub: Vec<i64> = context_activation(&context_events(&events, Some("agent-a")), &[(5, 1.0)]).into_keys().collect();
        sub.sort();
        assert_eq!(sub, [2, 4, 5], "a subagent never resets and never sees the main session's cues");
    }
}
