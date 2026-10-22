//! Recall during work (milestone 3b): what the `PostToolUse` /
//! `PostToolUseFailure` hooks do with a work cue, kept apart from
//! recording so `agentops work-recall-eval --replay` runs the same code
//! against an in-memory event log.
//!
//! Per cue ([`on_cue`]): resolve it (`agentops_retrieval::work_recall::
//! resolve_cue`), record its candidates as one `work_cue` event, compute
//! session activation from the context's recent `work_cue` events (ADR
//! 0053), and show at most `WORK_RECALL_MAX_ITEMS` memories the cue reached
//! whose activation is at least `INJECT_MIN_SCORE`, never one this context
//! has already been shown. Most cues show nothing. Only a file's first cue
//! in a context can show anything: a re-read or a later edit re-records
//! the earlier candidates for activation and stops there.
//!
//! **A context** is the main session, or one subagent (`agent_id`): each
//! has its own context window, so each has its own activation. The main
//! context starts over after a compaction (`context_reset`, recorded by
//! `SessionStart` with `source: compact`); a subagent's never does, since
//! Claude Code gives no signal for it.

use std::collections::{HashMap, HashSet};

use agentops_graph::{GraphStore, SessionEvent};
use agentops_retrieval::work_recall::{activation, resolve_cue, CueSources, WorkCandidate, WorkCue, ACTIVATION_DECAY, ACTIVATION_WINDOW};
use anyhow::Result;

use crate::budget::{INJECT_MIN_SCORE, MAX_INJECTED_PER_CONTEXT, SUBAGENT_MAX_INJECTED, WORK_RECALL_BUDGET, WORK_RECALL_MAX_ITEMS};
use crate::hooks::RECALL_INJECTED;

/// `tool_name` of a work-time `recall_injected` event, which the
/// per-context ceiling counts (prompt and subagent seeds don't).
pub const WORK_RECALL_TOOL: &str = "work_recall";
/// First line of every work-time injection, file or error.
pub const WORK_RECALL_HEADER: &str = "AgentOps recall; `fetch_content` <id> for the full text:";

/// A session's event log as work-time recall reads and writes it: the
/// store for a live hook, memory for `work-recall-eval --replay`, so the
/// replay runs this code and records nothing.
pub trait EventLog {
    /// Every event of the session, oldest first.
    fn events(&self) -> Result<Vec<SessionEvent>>;
    fn record(&mut self, tool_name: &str, description: &str, event_kind: &str, agent_id: Option<&str>) -> Result<()>;
}

/// The live hook's log: one session in the store.
pub struct StoreLog<'a> {
    pub store: &'a dyn GraphStore,
    pub repo: &'a str,
    pub session_id: &'a str,
}

impl EventLog for StoreLog<'_> {
    fn events(&self) -> Result<Vec<SessionEvent>> {
        self.store.session_events(self.repo, self.session_id)
    }
    fn record(&mut self, tool_name: &str, description: &str, event_kind: &str, agent_id: Option<&str>) -> Result<()> {
        self.store.record_session_event_in(self.repo, self.session_id, tool_name, description, None, event_kind, agent_id).map(|_| ())
    }
}

/// An in-memory log, for the replay.
#[derive(Default)]
pub struct MemoryLog(pub Vec<SessionEvent>);

impl EventLog for MemoryLog {
    fn events(&self) -> Result<Vec<SessionEvent>> {
        Ok(self.0.clone())
    }
    fn record(&mut self, tool_name: &str, description: &str, event_kind: &str, agent_id: Option<&str>) -> Result<()> {
        let id = self.0.len() as i64 + 1;
        self.0.push(SessionEvent { id, repo: String::new(), session_id: String::new(), tool_name: tool_name.into(), description: description.into(), node_id: None, event_kind: event_kind.into(), created_at: String::new(), agent_id: agent_id.map(String::from) });
        Ok(())
    }
}

/// What one cue showed: the context to inject and, per memory, its
/// activation when shown (the replay's labelling file reads it).
#[derive(Debug, Clone, PartialEq)]
pub struct Injection {
    pub context: String,
    pub shown: Vec<(agentops_retrieval::RecallPointer, f64)>,
}

/// The threshold, the off switch and the cue sources, which the replay
/// varies (the sources only for the ablation; the hook always runs all).
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub min_score: f64,
    pub enabled: bool,
    pub sources: CueSources,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { min_score: INJECT_MIN_SCORE, enabled: true, sources: CueSources::All }
    }
}

/// Node ids this context has been shown: every `recall_injected` in it
/// (prompt seeds, a subagent's seed, earlier work-time injections).
pub fn shown_ids(context: &[&SessionEvent]) -> HashSet<i64> {
    context.iter().filter(|e| e.event_kind == RECALL_INJECTED).flat_map(|e| crate::hooks::parse_injected(&e.description)).collect()
}

/// Work-time pointers this context has had, for `MAX_INJECTED_PER_CONTEXT`.
fn work_injected(context: &[&SessionEvent]) -> usize {
    context.iter().filter(|e| e.event_kind == RECALL_INJECTED && e.tool_name == WORK_RECALL_TOOL).map(|e| crate::hooks::parse_injected(&e.description).len()).sum()
}

/// One work cue in `agent_id`'s context: resolve, record, activate, and
/// maybe show (see the module docs). `None` when nothing is shown; with
/// the off switch set, the cue is still resolved and recorded.
pub fn on_cue(store: &dyn GraphStore, repo: &str, log: &mut dyn EventLog, agent_id: Option<&str>, cue: &WorkCue, embedder: Option<&'static (dyn agentops_embeddings::Embedder + Sync)>, settings: Settings) -> Result<Option<Injection>> {
    let key = cue_key(cue);
    let events = log.events()?;
    let context = context_events(&events, agent_id);
    // Only a file's first cue in a context injects (milestone 3b revision
    // 1): a re-read or a later edit moves the file's candidates to the
    // newest position for activation and shows nothing, without resolving
    // again. Before this, each re-read of a hub file drained its mention
    // list three pointers at a time (gotcha 235636).
    if let WorkCue::File { .. } = cue {
        if let Some(earlier) = context.iter().rev().find(|e| e.event_kind == WORK_CUE && e.tool_name == key) {
            let description = earlier.description.clone();
            log.record(&key, &description, WORK_CUE, agent_id)?;
            return Ok(None);
        }
    }
    let candidates = resolve_cue(store, repo, cue, settings.sources, embedder)?;
    if candidates.is_empty() {
        // A file that reached nothing is still recorded, so a re-read of it
        // is skipped above rather than resolved again.
        if let WorkCue::File { .. } = cue {
            log.record(&key, "", WORK_CUE, agent_id)?;
        }
        return Ok(None);
    }
    let scores = cue_scores(&candidates);
    let activated = context_activation(&context, &key, &scores);
    log.record(&key, &format_cue(&scores), WORK_CUE, agent_id)?;
    if !settings.enabled {
        return Ok(None);
    }
    // Past the ceiling, only an error still speaks in the main session; a
    // subagent stops at `SUBAGENT_MAX_INJECTED`, errors included. The
    // ceiling spans the whole context, compactions included.
    let whole: Vec<&SessionEvent> = events.iter().filter(|e| e.agent_id.as_deref() == agent_id).collect();
    let injected = work_injected(&whole);
    let room = match agent_id {
        Some(_) => SUBAGENT_MAX_INJECTED.saturating_sub(injected),
        None if matches!(cue, WorkCue::File { .. }) && injected >= MAX_INJECTED_PER_CONTEXT => 0,
        None => WORK_RECALL_MAX_ITEMS,
    };
    if room == 0 {
        return Ok(None);
    }
    let shown = shown_ids(&context);
    let mut picks: Vec<(&WorkCandidate, f64)> = candidates.iter().map(|c| (c, activated.get(&c.pointer.node_id).copied().unwrap_or(0.0))).filter(|(c, a)| *a >= settings.min_score && !shown.contains(&c.pointer.node_id)).collect();
    picks.sort_by(|a, b| b.1.total_cmp(&a.1));
    picks.truncate(WORK_RECALL_MAX_ITEMS.min(room));
    if picks.is_empty() {
        return Ok(None);
    }
    // A parallel tool call may have shown one of these meanwhile
    // (PostToolUse runs concurrently); this narrows that race, it can't
    // close it (findings.md).
    let now_shown = shown_ids(&context_events(&log.events()?, agent_id));
    picks.retain(|(c, _)| !now_shown.contains(&c.pointer.node_id));

    // The header rides on the tool call that triggered it, so it needn't
    // name the file or the error again; the lines skip the match reasons.
    // Together that halves each injection (milestone 3b revision 2).
    let mut text = WORK_RECALL_HEADER.to_string();
    let mut shown_now = Vec::new();
    for (candidate, activation) in picks {
        let line = candidate.pointer.render_short();
        if text.chars().count() + 1 + line.chars().count() > WORK_RECALL_BUDGET {
            break;
        }
        text.push('\n');
        text.push_str(&line);
        shown_now.push((candidate.pointer.clone(), activation));
    }
    if shown_now.is_empty() {
        return Ok(None);
    }
    let ids = shown_now.iter().map(|(p, _)| p.node_id.to_string()).collect::<Vec<_>>().join(",");
    log.record(WORK_RECALL_TOOL, &format!("ids {ids}"), RECALL_INJECTED, agent_id)?;
    Ok(Some(Injection { context: text, shown: shown_now }))
}

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

/// A `work_cue` event's `tool_name`: what the cue was, so a file read
/// again counts once (`file:<path>`). Errors, prompts and older events
/// never merge.
pub fn cue_key(cue: &WorkCue) -> String {
    match cue {
        WorkCue::File { path, .. } => format!("file:{path}"),
        WorkCue::Error { .. } => "error".to_string(),
    }
}

/// Session activation for a context whose events are `context` (see
/// `context_events`), plus `current`, the cue being handled now under
/// `current_key`, which isn't recorded yet. A file cued again counts once,
/// at its latest position: re-reading one file is one cue, not convergent
/// evidence (the first labelling replay had re-reads lift a file's every
/// memory past 1.0).
pub fn context_activation(context: &[&SessionEvent], current_key: &str, current: &[(i64, f64)]) -> HashMap<i64, f64> {
    let mut keyed: Vec<(&str, Vec<(i64, f64)>)> = context.iter().filter(|e| e.event_kind == WORK_CUE).map(|e| (e.tool_name.as_str(), parse_cue(&e.description))).collect();
    if !current.is_empty() {
        keyed.push((current_key, current.to_vec()));
    }
    let mut seen = HashSet::new();
    let mut cues: Vec<Vec<(i64, f64)>> = keyed
        .into_iter()
        .rev()
        .filter(|(key, _)| !key.starts_with("file:") || seen.insert(*key))
        .map(|(_, scores)| scores)
        .collect();
    cues.reverse();
    activation(&cues, ACTIVATION_WINDOW, ACTIVATION_DECAY)
}

/// One step of a past session, as `work-recall-eval --replay` feeds it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "step", rename_all = "lowercase")]
pub enum ReplayStep {
    /// A file read or edited, or a failed call, in `agent_id`'s context.
    Cue {
        #[serde(default)]
        agent_id: Option<String>,
        cue: WorkCue,
    },
    /// A user prompt (main context), in plan mode or not.
    Prompt { prompt: String, plan: bool },
    /// The main context was compacted.
    Compact,
}

/// The replay's event log on the wire: what `MemoryLog` holds, minus
/// fields the replay never reads.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoggedEvent {
    pub tool_name: String,
    pub description: String,
    pub event_kind: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

impl MemoryLog {
    pub fn from_logged(events: &[LoggedEvent]) -> Self {
        let mut log = MemoryLog::default();
        for e in events {
            let _ = log.record(&e.tool_name, &e.description, &e.event_kind, e.agent_id.as_deref());
        }
        log
    }

    pub fn to_logged(&self) -> Vec<LoggedEvent> {
        self.0.iter().map(|e| LoggedEvent { tool_name: e.tool_name.clone(), description: e.description.clone(), event_kind: e.event_kind.clone(), agent_id: e.agent_id.clone() }).collect()
    }
}

/// A prompt as the `UserPromptSubmit` hook treats it for work-time recall:
/// its seed is shown in the main context and seeds activation. (The hook
/// also cuts the seed to its budget; the replay shows all of it.)
pub fn on_prompt(store: &dyn GraphStore, repo: &str, log: &mut dyn EventLog, prompt: &str, plan: bool, embedder: Option<&'static (dyn agentops_embeddings::Embedder + Sync)>) -> Result<()> {
    use crate::budget::{DEFAULT_SEED_MAX_ITEMS, PLAN_SEED_MAX_ITEMS};
    let events = log.events()?;
    let exclude = if plan { HashSet::new() } else { shown_ids(&context_events(&events, None)) };
    let seed = agentops_retrieval::recall_scored_with(store, repo, prompt, if plan { PLAN_SEED_MAX_ITEMS } else { DEFAULT_SEED_MAX_ITEMS }, &exclude, embedder)?;
    if seed.is_empty() {
        return Ok(());
    }
    let scores: Vec<(i64, f64)> = seed.iter().map(|(p, s)| (p.node_id, *s)).collect();
    log.record("prompt", &format_cue(&scores), WORK_CUE, None)?;
    log.record("recall", &format!("ids {}", scores.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>().join(",")), RECALL_INJECTED, None)?;
    Ok(())
}

/// What one replayed step showed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplayInjection {
    /// Index of the step in the request.
    pub step: usize,
    pub agent_id: Option<String>,
    pub cue: WorkCue,
    pub chars: usize,
    /// `(node id, kind, title, reason, activation)` per pointer shown.
    pub shown: Vec<(i64, String, String, String, f64)>,
}

/// Runs `steps` in order against `log` with the hook's own code (`on_cue`,
/// `on_prompt`), recording only into `log`.
pub fn replay(store: &dyn GraphStore, repo: &str, log: &mut MemoryLog, steps: &[ReplayStep], embedder: Option<&'static (dyn agentops_embeddings::Embedder + Sync)>, settings: Settings) -> Result<Vec<ReplayInjection>> {
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        match step {
            ReplayStep::Compact => log.record("SessionStart", "compact", CONTEXT_RESET, None)?,
            ReplayStep::Prompt { prompt, plan } => on_prompt(store, repo, log, prompt, *plan, embedder)?,
            ReplayStep::Cue { agent_id, cue } => {
                let Some(injection) = on_cue(store, repo, log, agent_id.as_deref(), cue, embedder, settings)? else { continue };
                let shown = injection.shown.iter().map(|(p, a)| (p.node_id, p.kind.as_db_str().to_string(), p.title.clone(), p.reason.clone(), *a)).collect();
                out.push(ReplayInjection { step: i, agent_id: agent_id.clone(), cue: cue.clone(), chars: injection.context.chars().count(), shown });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, description: &str, agent: Option<&str>) -> SessionEvent {
        SessionEvent { id: 0, repo: "r".into(), session_id: "s".into(), tool_name: "t".into(), description: description.into(), node_id: None, event_kind: kind.into(), created_at: String::new(), agent_id: agent.map(String::from) }
    }

    fn gotcha(store: &dyn GraphStore, repo: &str, name: &str, content: &str) -> i64 {
        agentops_graph::upsert_node(store, agentops_graph::NewNode { kind: agentops_graph::NodeKind::Gotcha, repo: repo.into(), path: Some(format!("vault:{name}")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap()
    }

    fn setup() -> (agentops_graph::SqliteGraphStore, i64) {
        let store = agentops_graph::SqliteGraphStore::open_in_memory().unwrap();
        let id = gotcha(&store, "r", "Release tagging needs the changelog bumped", "Tagging a release fails unless docs/release.md has the changelog entry.");
        (store, id)
    }

    fn release_cue() -> WorkCue {
        WorkCue::File { path: "docs/release.md".into() }
    }

    const LOW: Settings = Settings { min_score: 0.1, enabled: true, sources: CueSources::All };

    #[test]
    fn a_memory_shown_once_is_never_shown_again_in_that_context_until_compaction() {
        let (store, id) = setup();
        let mut log = MemoryLog::default();
        let first = on_cue(&store, "r", &mut log, None, &release_cue(), None, LOW).unwrap().expect("shown");
        assert_eq!(first.shown.iter().map(|(p, _)| p.node_id).collect::<Vec<_>>(), [id]);
        assert!(first.context.starts_with(WORK_RECALL_HEADER) && first.context.ends_with(&format!("(id {id})")), "short lines, no reason: {}", first.context);
        assert!(first.context.chars().count() <= WORK_RECALL_BUDGET);
        assert_eq!(on_cue(&store, "r", &mut log, None, &release_cue(), None, LOW).unwrap(), None, "never twice");

        log.record("SessionStart", "compact", CONTEXT_RESET, None).unwrap();
        assert!(on_cue(&store, "r", &mut log, None, &release_cue(), None, LOW).unwrap().is_some(), "after compaction it can be");
    }

    #[test]
    fn only_a_files_first_cue_in_a_context_injects_until_compaction() {
        let (store, _) = setup();
        let mut log = MemoryLog::default();
        assert!(on_cue(&store, "r", &mut log, None, &release_cue(), None, LOW).unwrap().is_some());
        // A new memory naming the file arrives; a re-read or an edit of the
        // same file still shows nothing, and is recorded for activation.
        gotcha(&store, "r", "Release changelog needs a date", "docs/release.md: the changelog entry needs a release date.");
        let edit = WorkCue::File { path: "docs/release.md".into() };
        assert_eq!(on_cue(&store, "r", &mut log, None, &release_cue(), None, LOW).unwrap(), None, "a re-read never injects");
        assert_eq!(on_cue(&store, "r", &mut log, None, &edit, None, LOW).unwrap(), None, "nor does a later edit");
        assert_eq!(log.0.iter().filter(|e| e.event_kind == WORK_CUE && e.tool_name == "file:docs/release.md").count(), 3, "each cue still counts for activation");

        log.record("SessionStart", "compact", CONTEXT_RESET, None).unwrap();
        let after = on_cue(&store, "r", &mut log, None, &release_cue(), None, LOW).unwrap().expect("the first cue after compaction injects");
        assert_eq!(after.shown.len(), WORK_RECALL_MAX_ITEMS, "only the cue's best match: {after:?}");
    }

    #[test]
    fn a_file_that_reached_nothing_is_not_resolved_again() {
        let (store, _) = setup();
        let mut log = MemoryLog::default();
        let nothing = WorkCue::File { path: "src/unrelated_widget.rs".into() };
        assert_eq!(on_cue(&store, "r", &mut log, None, &nothing, None, LOW).unwrap(), None);
        gotcha(&store, "r", "Unrelated widget panics", "src/unrelated_widget.rs panics on empty input.");
        assert_eq!(on_cue(&store, "r", &mut log, None, &nothing, None, LOW).unwrap(), None, "the earlier empty cue counts as the file's first");
    }

    #[test]
    fn a_memory_shown_by_a_seed_is_skipped_and_contexts_never_share_a_shown_set() {
        let (store, id) = setup();
        let mut log = MemoryLog::default();
        log.record("recall", &format!("ids {id}"), RECALL_INJECTED, None).unwrap();
        assert_eq!(on_cue(&store, "r", &mut log, None, &release_cue(), None, LOW).unwrap(), None, "the prompt seed showed it");
        assert!(on_cue(&store, "r", &mut log, Some("agent-a"), &release_cue(), None, LOW).unwrap().is_some(), "a subagent's context hasn't seen it");
        assert!(on_cue(&store, "r", &mut log, Some("agent-b"), &release_cue(), None, LOW).unwrap().is_some(), "nor another subagent's");
        assert_eq!(on_cue(&store, "r", &mut log, Some("agent-a"), &release_cue(), None, LOW).unwrap(), None);
    }

    #[test]
    fn past_the_ceiling_only_error_cues_inject_and_the_off_switch_still_records() {
        let (store, _) = setup();
        let psql = gotcha(&store, "r", "psql role postgres missing on CI", "psql fails: role postgres does not exist on the CI database.");
        let mut log = MemoryLog::default();
        let full = (0..MAX_INJECTED_PER_CONTEXT as i64).map(|i| (1000 + i).to_string()).collect::<Vec<_>>().join(",");
        log.record(WORK_RECALL_TOOL, &format!("ids {full}"), RECALL_INJECTED, None).unwrap();
        assert_eq!(on_cue(&store, "r", &mut log, None, &release_cue(), None, LOW).unwrap(), None, "a file cue past the ceiling");
        let error = WorkCue::Error { text: "psql: error: FATAL: role \"postgres\" does not exist".into() };
        assert_eq!(on_cue(&store, "r", &mut log, None, &error, None, LOW).unwrap().unwrap().shown[0].0.node_id, psql, "an error still does");

        let mut off = MemoryLog::default();
        assert_eq!(on_cue(&store, "r", &mut off, None, &release_cue(), None, Settings { enabled: false, ..LOW }).unwrap(), None);
        assert!(off.0.iter().any(|e| e.event_kind == WORK_CUE), "the cue is still recorded");
    }

    #[test]
    fn a_subagent_stops_at_its_own_ceiling_errors_included() {
        let (store, _) = setup();
        let psql = gotcha(&store, "r", "psql role postgres missing on CI", "psql fails: role postgres does not exist on the CI database.");
        let mut log = MemoryLog::default();
        let almost = (0..SUBAGENT_MAX_INJECTED as i64 - 1).map(|i| (1000 + i).to_string()).collect::<Vec<_>>().join(",");
        log.record(WORK_RECALL_TOOL, &format!("ids {almost}"), RECALL_INJECTED, Some("agent-a")).unwrap();
        gotcha(&store, "r", "Release notes list the tag", "docs/release.md: the notes list the changelog tag.");
        let shown = on_cue(&store, "r", &mut log, Some("agent-a"), &release_cue(), None, LOW).unwrap().expect("one slot left");
        assert_eq!(shown.shown.len(), 1, "never past the ceiling: {shown:?}");
        let error = WorkCue::Error { text: "psql: error: FATAL: role \"postgres\" does not exist".into() };
        assert_eq!(on_cue(&store, "r", &mut log, Some("agent-a"), &error, None, LOW).unwrap(), None, "a subagent's ceiling holds for errors too");
        assert_eq!(on_cue(&store, "r", &mut log, None, &error, None, LOW).unwrap().unwrap().shown[0].0.node_id, psql, "the main session has its own");
    }

    #[test]
    fn below_the_threshold_nothing_is_shown() {
        let (store, _) = setup();
        let mut log = MemoryLog::default();
        assert_eq!(on_cue(&store, "r", &mut log, None, &release_cue(), None, Settings { min_score: 10.0, ..LOW }).unwrap(), None);
        assert!(log.0.iter().all(|e| e.event_kind != RECALL_INJECTED), "silence records no injection");
    }

    #[test]
    fn a_file_read_again_counts_once_but_different_cues_add_up() {
        let mut log = MemoryLog::default();
        for _ in 0..4 {
            log.record("file:a.rs", "1:0.5", WORK_CUE, None).unwrap();
        }
        let events = log.events().unwrap();
        let again = context_activation(&context_events(&events, None), "file:a.rs", &[(1, 0.5)]);
        assert!((again[&1] - 0.5).abs() < 1e-9, "five reads of one file are one cue: {again:?}");
        let other = context_activation(&context_events(&events, None), "file:b.rs", &[(1, 0.5)]);
        assert!(other[&1] > 0.9, "a second file reaching it adds up: {other:?}");
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
        let main = context_activation(&context_events(&events, None), "x", &[]);
        assert_eq!(main.keys().copied().collect::<Vec<_>>(), [3], "main sees only its own cues after its compaction");
        let mut sub: Vec<i64> = context_activation(&context_events(&events, Some("agent-a")), "file:y", &[(5, 1.0)]).into_keys().collect();
        sub.sort();
        assert_eq!(sub, [2, 4, 5], "a subagent never resets and never sees the main session's cues");
    }
}
