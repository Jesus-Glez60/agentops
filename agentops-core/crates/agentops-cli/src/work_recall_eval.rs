//! `agentops work-recall-eval`: measures recall during work (milestone 3b,
//! item 0). Three modes:
//!
//! - `--sizing`: per-session counts of the tool calls a work-time hook
//!   would see, from this repo's Claude Code transcripts (main sessions and
//!   subagents counted separately). Sets how often the hook fires.
//! - cases (the default): each `{id, cue: {kind: file|error, path?, text?},
//!   targets}` in `.context/work-recall-eval.jsonl` runs as one cue through
//!   `agentops_retrieval::work_recall::resolve_cue`, the resolution the
//!   `PostToolUse` hook runs, locally or through the server's
//!   `POST /repos/{id}/work-recall` with `--remote`. Nothing is recorded.
//!   A hit is a target within `WORK_RECALL_MAX_ITEMS`. `--baseline affects`
//!   limits resolution to `Affects` edges (what `get_symbol` gives today);
//!   `--baseline prompt` feeds the cue's text to prompt-time recall instead.
//! - `--replay <session>...`: walks past sessions' tool calls in order and
//!   reports the cues each produces (injections arrive with item 4).
//!
//! Cases, paths and memory titles stay under `.context/`: only summary
//! numbers are printed for the repo.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use agentops_graph::NodeProminence;
use agentops_retrieval::work_recall::{bash_read_path, is_skipped, CueSources, WorkCue};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use agentops_mcp::work_recall::{ReplayInjection, ReplayStep};

use crate::recall_eval::{percentile, remote_send, remote_send_within, unix_secs, Backend, DEEP_LIMIT};
use crate::remote_marker::read_remote_marker;

/// Transcripts older than this aren't sized (the spec's 30 days).
const SIZING_WINDOW: Duration = Duration::from_secs(30 * 86_400);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Baseline {
    /// Only memories `Affects`-linked to the file's symbols.
    Affects,
    /// The cue's text (path or error) as a prompt to prompt-time recall.
    Prompt,
}

// ---------------------------------------------------------------------
// Transcripts
// ---------------------------------------------------------------------

/// One tool call in a transcript, with how it ended.
#[derive(Debug, Clone)]
pub(crate) struct ToolCall {
    pub(crate) tool: String,
    pub(crate) input: Value,
    pub(crate) failed: bool,
    /// The tool result's text (the error, for a failed call).
    pub(crate) result: String,
}

/// One transcript: a main session, or one of its subagents.
#[derive(Debug)]
pub(crate) struct Transcript {
    pub(crate) session_id: String,
    /// `None` for the main session; the subagent's file stem otherwise.
    pub(crate) agent_id: Option<String>,
    pub(crate) calls: Vec<ToolCall>,
}

/// The text of a tool result's `content`: a string, or the `text` of each
/// block.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Every tool call in one JSONL transcript, in call order, each matched to
/// its result by `tool_use_id`. A message split over several lines repeats
/// earlier blocks, so calls are deduped by id. Unparseable lines (a
/// trailing partial write) are skipped.
pub(crate) fn read_tool_calls(path: &Path) -> Result<Vec<ToolCall>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut order: Vec<String> = Vec::new();
    let mut calls: HashMap<String, ToolCall> = HashMap::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        let Some(blocks) = entry.pointer("/message/content").and_then(Value::as_array) else { continue };
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    let Some(id) = block.get("id").and_then(Value::as_str) else { continue };
                    if calls.contains_key(id) {
                        continue;
                    }
                    order.push(id.to_string());
                    calls.insert(id.to_string(), ToolCall { tool: block.get("name").and_then(Value::as_str).unwrap_or_default().to_string(), input: block.get("input").cloned().unwrap_or(Value::Null), failed: false, result: String::new() });
                }
                Some("tool_result") => {
                    let Some(call) = block.get("tool_use_id").and_then(Value::as_str).and_then(|id| calls.get_mut(id)) else { continue };
                    call.failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    call.result = result_text(block.get("content").unwrap_or(&Value::Null));
                }
                _ => {}
            }
        }
    }
    Ok(order.into_iter().filter_map(|id| calls.remove(&id)).collect())
}

/// This repo's transcripts modified within `window` (all of them for
/// `None`), main sessions and their `<session>/subagents/*.jsonl`.
pub(crate) fn read_transcripts(repo_path: &Path, claude_home: Option<&Path>, window: Option<Duration>, sessions: Option<&[String]>) -> Result<Vec<Transcript>> {
    let canonical = std::fs::canonicalize(repo_path).with_context(|| format!("resolving {}", repo_path.display()))?;
    let home = claude_home.map(Path::to_path_buf).unwrap_or_else(crate::usage::default_claude_home);
    let dir = home.join("projects").join(crate::usage::sanitize_project_path(&canonical));
    let recent = |p: &Path| window.is_none_or(|w| std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() <= w));
    let wanted = |id: &str| sessions.is_none_or(|s| s.iter().any(|prefix| id.starts_with(prefix.as_str())));
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(out) };
    let mut mains: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
    mains.sort();
    for main in mains {
        let session_id = main.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        if !wanted(&session_id) {
            continue;
        }
        if recent(&main) {
            out.push(Transcript { session_id: session_id.clone(), agent_id: None, calls: read_tool_calls(&main)? });
        }
        let Ok(subs) = std::fs::read_dir(main.with_extension("").join("subagents")) else { continue };
        let mut subs: Vec<PathBuf> = subs.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
        subs.sort();
        for sub in subs.into_iter().filter(|p| recent(p)) {
            out.push(Transcript { session_id: session_id.clone(), agent_id: Some(sub.file_stem().unwrap_or_default().to_string_lossy().into_owned()), calls: read_tool_calls(&sub)? });
        }
    }
    Ok(out)
}

/// Failures Claude Code never sends to `PostToolUseFailure`: input
/// validation errors and permission denials (hooks reference), and a user
/// declining the call.
fn reaches_failure_hook(result: &str) -> bool {
    !(result.contains("<tool_use_error>") || result.starts_with("Permission for this action was denied") || result.contains("doesn't want to proceed") || result.contains("user rejected"))
}

/// The work cue a tool call produces, as the hook will see it: a file read
/// or edited inside the repo and off the skip list, or a failure that
/// reaches `PostToolUseFailure`.
pub(crate) fn cue_of(call: &ToolCall, root: &Path) -> Option<WorkCue> {
    if call.failed {
        return reaches_failure_hook(&call.result).then(|| WorkCue::Error { text: call.result.clone() });
    }
    if !matches!(call.tool.as_str(), "Read" | "NotebookRead" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Bash") {
        return None;
    }
    // The hook's own path logic; a transcript keeps no per-call cwd, so a
    // relative path is taken from the repo root.
    let path = crate::hook_capture::touched_rel_path(&json!({ "tool_input": call.input }), root, root)?;
    (!is_skipped(&path)).then_some(WorkCue::File { path })
}

// ---------------------------------------------------------------------
// Sizing
// ---------------------------------------------------------------------

/// Per-transcript counts.
#[derive(Debug, Default, Serialize)]
struct Counts {
    read: usize,
    bash_read: usize,
    edit: usize,
    write: usize,
    grep: usize,
    glob: usize,
    failed: usize,
    error_cues: usize,
    file_cues: usize,
    distinct_files: usize,
    calls: usize,
}

fn count(transcript: &Transcript, root: &Path) -> Counts {
    let mut c = Counts { calls: transcript.calls.len(), ..Default::default() };
    let mut files = HashSet::new();
    for call in &transcript.calls {
        match call.tool.as_str() {
            "Read" | "NotebookRead" => c.read += 1,
            "Edit" | "MultiEdit" | "NotebookEdit" => c.edit += 1,
            "Write" => c.write += 1,
            "Grep" => c.grep += 1,
            "Glob" => c.glob += 1,
            "Bash" if !call.failed && call.input.get("command").and_then(Value::as_str).and_then(bash_read_path).is_some() => c.bash_read += 1,
            _ => {}
        }
        c.failed += usize::from(call.failed);
        match cue_of(call, root) {
            Some(WorkCue::Error { .. }) => c.error_cues += 1,
            Some(WorkCue::File { path, .. }) => {
                c.file_cues += 1;
                files.insert(path);
            }
            None => {}
        }
    }
    c.distinct_files = files.len();
    c
}

/// `{median, p95, max}` of one count over a set of transcripts.
fn spread(values: &[f64]) -> Value {
    json!({ "median": percentile(values, 50.0), "p95": percentile(values, 95.0), "max": values.iter().copied().fold(0.0, f64::max) })
}

fn sizing(path: &Path, root: &Path) -> Result<()> {
    let transcripts = read_transcripts(path, None, Some(SIZING_WINDOW), None)?;
    let mut report = serde_json::Map::new();
    let mut table = String::new();
    for (label, main) in [("main", true), ("subagent", false)] {
        let counts: Vec<Counts> = transcripts.iter().filter(|t| t.agent_id.is_none() == main).map(|t| count(t, root)).filter(|c| c.calls > 0).collect();
        let column = |f: fn(&Counts) -> usize| counts.iter().map(|c| f(c) as f64).collect::<Vec<_>>();
        let fields: [(&str, fn(&Counts) -> usize); 11] = [
            ("read", |c| c.read),
            ("bash_read", |c| c.bash_read),
            ("edit", |c| c.edit),
            ("write", |c| c.write),
            ("grep", |c| c.grep),
            ("glob", |c| c.glob),
            ("failed", |c| c.failed),
            ("error_cues", |c| c.error_cues),
            ("file_cues", |c| c.file_cues),
            ("distinct_files", |c| c.distinct_files),
            ("calls", |c| c.calls),
        ];
        let mut section = serde_json::Map::new();
        section.insert("transcripts".into(), json!(counts.len()));
        table.push_str(&format!("{label}: {} transcripts with tool calls (last 30 days)\n", counts.len()));
        for (name, f) in fields {
            let values = column(f);
            let s = spread(&values);
            table.push_str(&format!("  {name:<15} median {:>5}  p95 {:>5}  max {:>5}  total {:>6}\n", s["median"], s["p95"], s["max"], values.iter().sum::<f64>()));
            section.insert(name.into(), s);
        }
        report.insert(label.into(), Value::Object(section));
    }
    print!("{table}");
    write_result(path, "sizing", &Value::Object(report))
}

// ---------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    cue: WorkCue,
    targets: Vec<i64>,
}

#[derive(Debug, Serialize)]
struct CaseResult {
    case_id: String,
    cue_kind: &'static str,
    /// Rank of the first target among the cue's candidates (up to
    /// `DEEP_LIMIT`), whether or not it made the injection limit.
    first_hit_rank: Option<usize>,
    hit: bool,
    latency_ms: f64,
}

#[derive(Debug, Serialize)]
struct Summary {
    cases: usize,
    hit_rate: f64,
    hits: usize,
    file_hits: usize,
    error_hits: usize,
    mrr: f64,
    p50_ms: f64,
    p95_ms: f64,
}

fn cue_kind(cue: &WorkCue) -> &'static str {
    match cue {
        WorkCue::File { .. } => "file",
        WorkCue::Error { .. } => "error",
    }
}

/// The candidate node ids `cue` reaches with `sources`, best first.
fn resolve(backend: &Backend, cue: &WorkCue, sources: CueSources) -> Result<Vec<i64>> {
    match backend {
        Backend::Local { store, repo, embedder } => Ok(agentops_retrieval::work_recall::resolve_cue(store.as_ref(), repo, cue, sources, *embedder)?.into_iter().take(DEEP_LIMIT).map(|c| c.pointer.node_id).collect()),
        Backend::Remote(marker) => {
            let reply = remote_send(marker, "work-recall", Some(&json!({ "cue": cue, "sources": sources, "limit": DEEP_LIMIT })))?;
            Ok(reply["candidates"].as_array().context("reading the work-recall route's candidates")?.iter().filter_map(|c| c["node_id"].as_i64()).collect())
        }
    }
}

/// The text a cue gives prompt-time recall in the `prompt` baseline.
fn cue_text(cue: &WorkCue) -> &str {
    match cue {
        WorkCue::File { path, .. } => path,
        WorkCue::Error { text } => text,
    }
}

fn read_cases(file: &Path) -> Result<Vec<Case>> {
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let cases: Vec<Case> = text.lines().filter(|l| !l.trim().is_empty()).enumerate().map(|(i, l)| serde_json::from_str(l).with_context(|| format!("{} line {}", file.display(), i + 1))).collect::<Result<_>>()?;
    if cases.is_empty() {
        bail!("{} has no cases", file.display());
    }
    Ok(cases)
}

/// Refuses a case no recall could pass for the wrong reason: a target that
/// doesn't exist or is never recalled, or a file case whose target a plain
/// `Affects` lookup already reaches (the spec wants those out).
fn validate(backend: &Backend, cases: &[Case]) -> Result<()> {
    for case in cases {
        if case.targets.is_empty() {
            bail!("case {}: no targets", case.id);
        }
        for &target in &case.targets {
            match backend.prominence(target)? {
                None => bail!("case {}: target {target} doesn't exist", case.id),
                Some(p @ (NodeProminence::Pinned | NodeProminence::NeedsReview)) => bail!("case {}: target {target} is {} and so never recalled; pick another", case.id, p.as_db_str()),
                Some(_) => {}
            }
        }
        if matches!(case.cue, WorkCue::File { .. }) {
            let linked: Vec<i64> = resolve(backend, &case.cue, CueSources::AffectsOnly)?.into_iter().filter(|id| case.targets.contains(id)).collect();
            if !linked.is_empty() {
                bail!("case {}: target(s) {linked:?} are Affects-linked to the file; a plain graph lookup already passes it", case.id);
            }
        }
    }
    Ok(())
}

fn evaluate(backend: &Backend, cases: &[Case], baseline: Option<Baseline>, repeat: usize, sources: CueSources) -> Result<(Vec<CaseResult>, Summary)> {
    let limit = agentops_mcp::budget::WORK_RECALL_MAX_ITEMS;
    let mut results = Vec::new();
    let mut latencies = Vec::new();
    for case in cases {
        let mut ranked = Vec::new();
        let mut runs = Vec::new();
        for _ in 0..repeat {
            let started = Instant::now();
            ranked = match baseline {
                Some(Baseline::Prompt) => backend.deep(cue_text(&case.cue))?.into_iter().map(|p| p.node_id).collect(),
                Some(Baseline::Affects) => resolve(backend, &case.cue, CueSources::AffectsOnly)?,
                None => resolve(backend, &case.cue, sources)?,
            };
            runs.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let rank = ranked.iter().position(|id| case.targets.contains(id)).map(|i| i + 1);
        latencies.extend(&runs);
        results.push(CaseResult { case_id: case.id.clone(), cue_kind: cue_kind(&case.cue), first_hit_rank: rank, hit: rank.is_some_and(|r| r <= limit), latency_ms: percentile(&runs, 50.0) });
    }
    let hits = |kind: &str| results.iter().filter(|r| r.hit && r.cue_kind == kind).count();
    let summary = Summary {
        cases: results.len(),
        hits: results.iter().filter(|r| r.hit).count(),
        hit_rate: results.iter().filter(|r| r.hit).count() as f64 / results.len() as f64,
        file_hits: hits("file"),
        error_hits: hits("error"),
        // MRR within the injection limit: a rank past it is a miss.
        mrr: results.iter().map(|r| r.first_hit_rank.filter(|&n| n <= limit).map_or(0.0, |n| 1.0 / n as f64)).sum::<f64>() / results.len() as f64,
        p50_ms: percentile(&latencies, 50.0),
        p95_ms: percentile(&latencies, 95.0),
    };
    Ok((results, summary))
}

fn render(results: &[CaseResult], summary: &Summary, label: &str) -> String {
    let mut out = format!("{label}\n{:<6} {:<6} {:>5} {:<4} {:>8}\n", "case", "cue", "rank", "hit", "p50 ms");
    for r in results {
        out.push_str(&format!("{:<6} {:<6} {:>5} {:<4} {:>8.0}\n", r.case_id, r.cue_kind, r.first_hit_rank.map_or("-".into(), |n| n.to_string()), if r.hit { "yes" } else { "no" }, r.latency_ms));
    }
    out.push_str(&format!(
        "\nhit rate {}/{} (file {}, error {}), MRR {:.3}, latency p50 {:.0} ms / p95 {:.0} ms\n",
        summary.hits, summary.cases, summary.file_hits, summary.error_hits, summary.mrr, summary.p50_ms, summary.p95_ms
    ));
    out
}

// ---------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------

/// Replay steps sent per request (each cue takes ~85 ms on the server).
const REPLAY_CHUNK: usize = 25;
/// Time one replay chunk may take.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(120);

/// One transcript as replay steps, in order: user prompts (main session
/// only), compactions, and a cue per tool call when its result arrives,
/// which is when `PostToolUse` fires. An error is redacted, as the hook
/// redacts it.
pub(crate) fn read_steps(path: &Path, root: &Path, agent_id: Option<&str>) -> Result<Vec<ReplayStep>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut calls: HashMap<String, (String, Value)> = HashMap::new();
    let mut done: HashSet<String> = HashSet::new();
    let mut steps = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        if entry.get("subtype").and_then(Value::as_str) == Some("compact_boundary") && agent_id.is_none() {
            steps.push(ReplayStep::Compact);
            continue;
        }
        let content = entry.pointer("/message/content");
        if let (Some(Value::String(prompt)), None, Some("user")) = (content, agent_id, entry.get("type").and_then(Value::as_str)) {
            let meta = entry.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
            if !meta && !prompt.trim().is_empty() && !prompt.trim_start().starts_with('<') {
                let plan = entry.get("permissionMode").and_then(Value::as_str) == Some("plan");
                steps.push(ReplayStep::Prompt { prompt: agentops_security::redact(prompt).text, plan });
            }
            continue;
        }
        let Some(blocks) = content.and_then(Value::as_array) else { continue };
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        calls.entry(id.to_string()).or_insert_with(|| (block.get("name").and_then(Value::as_str).unwrap_or_default().to_string(), block.get("input").cloned().unwrap_or(Value::Null)));
                    }
                }
                Some("tool_result") => {
                    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else { continue };
                    let Some((tool, input)) = calls.get(id) else { continue };
                    if !done.insert(id.to_string()) {
                        continue;
                    }
                    let call = ToolCall { tool: tool.clone(), input: input.clone(), failed: block.get("is_error").and_then(Value::as_bool).unwrap_or(false), result: result_text(block.get("content").unwrap_or(&Value::Null)) };
                    let cue = match cue_of(&call, root) {
                        Some(cue @ WorkCue::File { .. }) => cue,
                        Some(WorkCue::Error { text }) => WorkCue::Error { text: agentops_security::redact(&text).text },
                        None => continue,
                    };
                    steps.push(ReplayStep::Cue { agent_id: agent_id.map(String::from), cue });
                }
                _ => {}
            }
        }
    }
    Ok(steps)
}

/// Per-session replay numbers.
#[derive(Debug, Default, Serialize)]
struct ReplaySession {
    session: String,
    events: usize,
    cues: usize,
    contexts: usize,
    injections: usize,
    chars_added: usize,
    distinct_memories: usize,
    /// The most work-time pointers any one context got.
    max_per_context: usize,
}

/// One labelling line: an injection's pointer with its cue. Node ids and
/// paths only, no memory text (the author labels it `useful`, `neutral`
/// or `noise`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LabelLine {
    session: String,
    context: String,
    step: usize,
    cue: String,
    node_id: i64,
    kind: String,
    /// For the `.md` view only; the file stays under `.context/`.
    title: String,
    activation: f64,
    label: String,
}

/// `n` pointers to label (milestone 3b revision 1, author's call): 40%
/// from error cues, 40% from the main session's file cues, 20% from
/// subagents' file cues, each group in a fixed pseudo-random order
/// (`DefaultHasher::new()` has fixed keys). A group short of its share is
/// topped up from the others' leftovers, in the same order.
fn sample(labels: &[LabelLine], n: usize) -> Vec<&LabelLine> {
    use std::hash::{Hash, Hasher};
    let order = |l: &&LabelLine| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&l.session, &l.context, l.step, l.node_id).hash(&mut h);
        h.finish()
    };
    let mut groups: [Vec<&LabelLine>; 3] = Default::default();
    for l in labels {
        groups[if l.cue == ERROR_CUE { 0 } else if l.context == "main" { 1 } else { 2 }].push(l);
    }
    let quotas = [n * 2 / 5, n * 2 / 5, n - 2 * (n * 2 / 5)];
    let mut picked = Vec::new();
    let mut leftovers = Vec::new();
    for (group, quota) in groups.iter_mut().zip(quotas) {
        group.sort_by_key(order);
        let cut = quota.min(group.len());
        picked.extend(group.drain(..cut));
        leftovers.append(group);
    }
    leftovers.sort_by_key(order);
    let room = n.saturating_sub(picked.len());
    picked.extend(leftovers.into_iter().take(room));
    picked
}

/// Node ids with a non-empty label in a labelling `.jsonl`, or `None` when
/// the file can't be read.
fn read_labelled_ids(file: &Path) -> Option<HashSet<i64>> {
    let text = std::fs::read_to_string(file).ok()?;
    Some(text.lines().filter_map(|l| serde_json::from_str::<LabelLine>(l).ok()).filter(|l| !l.label.is_empty()).map(|l| l.node_id).collect())
}

/// The labelling sample as a Markdown table, numbered as in the `.jsonl`.
fn sample_markdown(sample: &[&LabelLine]) -> String {
    let mut out = String::from("# Work-time recall labels (local only, git-ignored)\n\nFill the `label` column in `to-label.jsonl` (useful / neutral / noise), or reply with the numbers, e.g. `noise: 3, 7-9; useful: 1, 4`.\n\nContext `main` = the main session; `sub` = a subagent. Cue = the file it opened, or (error).\n\n| n | ctx | cue | memory (id) | act |\n|---|---|---|---|---|\n");
    for (i, l) in sample.iter().enumerate() {
        let ctx = if l.context == "main" { "main" } else { "sub" };
        let cue = if l.cue == ERROR_CUE { l.cue.clone() } else { format!("`{}`", l.cue) };
        let title: String = l.title.replace('|', "/").chars().take(100).collect();
        out.push_str(&format!("| {} | {ctx} | {cue} | [{}] {title} ({}) | {:.2} |\n", i + 1, l.kind, l.node_id, l.activation));
    }
    out
}

/// The `cue` of a labelling line for an error cue.
const ERROR_CUE: &str = "(error)";

fn replay(path: &Path, root: &Path, sessions: &[String], remote: bool, min_score: Option<f64>, sources: CueSources, sample_size: Option<usize>) -> Result<()> {
    let transcripts = read_transcripts(path, None, None, Some(sessions))?;
    for prefix in sessions {
        if !transcripts.iter().any(|t| t.session_id.starts_with(prefix.as_str())) {
            bail!("no transcript for session {prefix}");
        }
    }
    let marker = if remote { Some(read_remote_marker(path).with_context(|| format!("--remote needs a server connection; {} has no .context/agentops-remote.json", path.display()))?) } else { None };
    let local = if remote { None } else { Some((agentops_mcp::open_store(path)?, agentops_mcp::repo_name(path))) };
    let home = crate::usage::default_claude_home().join("projects").join(crate::usage::sanitize_project_path(root));

    let mut summaries: BTreeMap<String, ReplaySession> = BTreeMap::new();
    let mut labels = Vec::new();
    for t in &transcripts {
        let file = match &t.agent_id {
            None => home.join(format!("{}.jsonl", t.session_id)),
            Some(agent) => home.join(&t.session_id).join("subagents").join(format!("{agent}.jsonl")),
        };
        let steps = read_steps(&file, root, t.agent_id.as_deref())?;
        let summary = summaries.entry(t.session_id.clone()).or_insert_with(|| ReplaySession { session: t.session_id.clone(), ..Default::default() });
        summary.events += t.calls.len();
        summary.contexts += 1;
        summary.cues += steps.iter().filter(|s| matches!(s, ReplayStep::Cue { .. })).count();

        // Contexts never share state, so each transcript replays on its own.
        let mut events: Vec<agentops_mcp::work_recall::LoggedEvent> = Vec::new();
        let mut shown_here = 0;
        for (chunk_no, chunk) in steps.chunks(REPLAY_CHUNK).enumerate() {
            let injections: Vec<ReplayInjection> = match (&marker, &local) {
                (Some(marker), _) => {
                    let reply = remote_send_within(marker, "work-recall/replay", Some(&json!({ "events": events, "steps": chunk, "min_score": min_score, "sources": sources })), REPLAY_TIMEOUT)?;
                    events = serde_json::from_value(reply["events"].clone()).context("reading the replay's events")?;
                    serde_json::from_value(reply["injections"].clone()).context("reading the replay's injections")?
                }
                (None, Some((store, repo))) => {
                    let mut log = agentops_mcp::work_recall::MemoryLog::from_logged(&events);
                    let mut settings = agentops_mcp::work_recall::Settings::default();
                    if let Some(min) = min_score {
                        settings.min_score = min;
                    }
                    settings.sources = sources;
                    let out = agentops_mcp::work_recall::replay(store.as_ref(), repo, &mut log, chunk, None, settings)?;
                    events = log.to_logged();
                    out
                }
                (None, None) => unreachable!(),
            };
            for injection in injections {
                summary.injections += 1;
                summary.chars_added += injection.chars;
                shown_here += injection.shown.len();
                let cue = match &injection.cue {
                    WorkCue::File { path, .. } => path.clone(),
                    WorkCue::Error { .. } => ERROR_CUE.to_string(),
                };
                for (node_id, kind, title, _reason, activation) in injection.shown {
                    labels.push(LabelLine { session: t.session_id.clone(), context: t.agent_id.clone().unwrap_or_else(|| "main".into()), step: chunk_no * REPLAY_CHUNK + injection.step, cue: cue.clone(), node_id, kind, title, activation, label: String::new() });
                }
            }
        }
        summary.max_per_context = summary.max_per_context.max(shown_here);
    }
    for summary in summaries.values_mut() {
        summary.distinct_memories = labels.iter().filter(|l| l.session == summary.session).map(|l| l.node_id).collect::<HashSet<_>>().len();
    }

    println!("{:<10} {:>6} {:>5} {:>8} {:>10} {:>6} {:>8} {:>8}", "session", "events", "cues", "contexts", "injections", "chars", "memories", "max/ctx");
    for s in summaries.values() {
        println!("{:<10} {:>6} {:>5} {:>8} {:>10} {:>6} {:>8} {:>8}", &s.session[..8.min(s.session.len())], s.events, s.cues, s.contexts, s.injections, s.chars_added, s.distinct_memories, s.max_per_context);
    }
    let dir = path.join(".context").join("work-recall-eval");
    std::fs::create_dir_all(&dir)?;
    let labels_file = dir.join(format!("labels-{}.jsonl", unix_secs()));
    std::fs::write(&labels_file, labels.iter().map(|l| serde_json::to_string(l).unwrap_or_default()).collect::<Vec<_>>().join("\n") + "\n")?;
    println!("Labelling file ({} pointers): {}", labels.len(), labels_file.display());
    if let Some(n) = sample_size {
        // A sample already labelled is kept as `labelled-<time>.jsonl`, and
        // no memory labelled in any earlier sample is drawn again, so a new
        // sample is held out from what earlier rules were fitted on
        // (milestone 3b revision 2).
        let current = dir.join("to-label.jsonl");
        if read_labelled_ids(&current).is_some_and(|ids| !ids.is_empty()) {
            std::fs::rename(&current, dir.join(format!("labelled-{}.jsonl", unix_secs())))?;
        }
        let mut seen = HashSet::new();
        for entry in std::fs::read_dir(&dir)?.flatten() {
            if entry.file_name().to_string_lossy().starts_with("labelled-") {
                seen.extend(read_labelled_ids(&entry.path()).unwrap_or_default());
            }
        }
        let fresh: Vec<LabelLine> = labels.iter().filter(|l| !seen.contains(&l.node_id)).cloned().collect();
        println!("Held out: {} of {} pointers reach a memory labelled before; the sample draws from the other {}.", labels.len() - fresh.len(), labels.len(), fresh.len());
        let picked = sample(&fresh, n);
        std::fs::write(dir.join("to-label.jsonl"), picked.iter().map(|l| serde_json::to_string(l).unwrap_or_default()).collect::<Vec<_>>().join("\n") + "\n")?;
        std::fs::write(dir.join("to-label.md"), sample_markdown(&picked))?;
        let errors = picked.iter().filter(|l| l.cue == ERROR_CUE).count();
        let main = picked.iter().filter(|l| l.context == "main" && l.cue != ERROR_CUE).count();
        println!("Labelling sample: {} of {} pointers ({errors} from errors, {main} main-session file cues, {} subagent file cues): {}", picked.len(), labels.len(), picked.len() - errors - main, dir.join("to-label.md").display());
    }
    write_result(path, "replay", &json!({ "min_score": min_score, "sources": sources, "sessions": summaries.values().collect::<Vec<_>>() }))
}

// ---------------------------------------------------------------------

fn write_result(path: &Path, kind: &str, value: &Value) -> Result<()> {
    let dir = path.join(".context").join("work-recall-eval");
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{kind}-{}.json", unix_secs()));
    std::fs::write(&out, serde_json::to_string_pretty(value)?)?;
    println!("Wrote {}", out.display());
    Ok(())
}

pub(crate) struct Options<'a> {
    pub(crate) remote: bool,
    /// Locally, resolve with the embedding model, as the server does.
    pub(crate) embed: bool,
    pub(crate) cases: Option<&'a Path>,
    pub(crate) baseline: Option<Baseline>,
    pub(crate) repeat: usize,
    pub(crate) sizing: bool,
    pub(crate) replay: &'a [String],
    pub(crate) min_score: Option<f64>,
    pub(crate) sources: Option<&'a str>,
    pub(crate) sample: Option<usize>,
}

pub(crate) fn run(path: &Path, options: Options) -> Result<()> {
    let root = std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    if options.sizing {
        return sizing(path, &root);
    }
    let sources: CueSources = match options.sources {
        Some(s) => serde_json::from_value(json!(s)).with_context(|| format!("--sources {s}: expected all, no_affects or no_mentions"))?,
        None => CueSources::All,
    };
    if !options.replay.is_empty() {
        return replay(path, &root, options.replay, options.remote, options.min_score, sources, options.sample);
    }
    let cases_file = options.cases.map(Path::to_path_buf).unwrap_or_else(|| path.join(".context").join("work-recall-eval.jsonl"));
    let cases = read_cases(&cases_file)?;
    anyhow::ensure!(!(options.remote && options.embed), "--embed is for a local graph; the server already embeds");
    let backend = if options.remote {
        Backend::Remote(read_remote_marker(path).with_context(|| format!("--remote needs a server connection; {} has no .context/agentops-remote.json", path.display()))?)
    } else {
        crate::recall_eval::local_backend(path, options.embed)?
    };
    validate(&backend, &cases)?;
    let (results, summary) = evaluate(&backend, &cases, options.baseline, options.repeat.max(1), sources)?;
    let label = match options.baseline {
        Some(Baseline::Affects) => "baseline: Affects lookup only",
        Some(Baseline::Prompt) => "baseline: cue text through prompt-time recall",
        None => "work-time recall",
    };
    print!("{}", render(&results, &summary, &format!("{label} (sources: {})", options.sources.unwrap_or("all"))));
    write_result(path, "cases", &json!({ "remote": options.remote, "baseline": options.baseline, "sources": sources, "cases": results, "summary": summary }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_labelled_rows_count_as_labelled() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("to-label.jsonl");
        let line = |id: i64, label: &str| serde_json::to_string(&LabelLine { session: "s".into(), context: "main".into(), step: 1, cue: "a.rs".into(), node_id: id, kind: "note".into(), title: "t".into(), activation: 1.0, label: label.into() }).unwrap();
        std::fs::write(&file, format!("{}\n{}\n", line(1, "noise"), line(2, ""))).unwrap();
        assert_eq!(read_labelled_ids(&file), Some(HashSet::from([1])));
        assert_eq!(read_labelled_ids(&dir.path().join("missing.jsonl")), None);
    }

    #[test]
    fn the_labelling_sample_is_stratified_fixed_and_tops_up_a_short_group() {
        let line = |context: &str, cue: &str, node_id: i64| LabelLine { session: "s".into(), context: context.into(), step: node_id as usize, cue: cue.into(), node_id, kind: "gotcha".into(), title: "t | x".into(), activation: 1.0, label: String::new() };
        let mut labels: Vec<LabelLine> = (0..60).map(|id| line("agent-a", ERROR_CUE, id)).collect();
        labels.extend((100..160).map(|id| line("main", "src/a.rs", id)));
        labels.extend((200..205).map(|id| line("agent-a", "src/b.rs", id)));
        let picked = sample(&labels, 100);
        assert_eq!(picked.len(), 100);
        let count = |f: &dyn Fn(&LabelLine) -> bool| picked.iter().filter(|l| f(l)).count();
        assert_eq!(count(&|l| l.context != "main" && l.cue != ERROR_CUE), 5, "the short subagent group gives all it has");
        assert_eq!(count(&|l| l.cue == ERROR_CUE) + count(&|l| l.context == "main"), 95, "the rest is topped up from the others");
        assert!(count(&|l| l.cue == ERROR_CUE) >= 40 && count(&|l| l.context == "main") >= 40);
        assert_eq!(picked.iter().map(|l| l.node_id).collect::<Vec<_>>(), sample(&labels, 100).iter().map(|l| l.node_id).collect::<Vec<_>>(), "the same sample every run");
        assert!(sample_markdown(&picked).contains("t / x"), "a title can't break the table");
    }

    fn write(dir: &Path, name: &str, lines: &[Value]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        p
    }

    fn call(tool: &str, input: Value, failed: bool, result: &str) -> ToolCall {
        ToolCall { tool: tool.into(), input, failed, result: result.into() }
    }

    #[test]
    fn tool_calls_are_read_in_order_once_each_with_their_result() {
        let dir = tempfile::tempdir().unwrap();
        let use_a = json!({"type": "tool_use", "id": "a", "name": "Read", "input": {"file_path": "/r/x.rs"}});
        let use_b = json!({"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "false"}});
        let path = write(
            dir.path(),
            "s.jsonl",
            &[
                json!({"message": {"content": [use_a.clone()]}}),
                json!({"message": {"content": [use_a, use_b]}}),
                json!({"message": {"content": [{"type": "tool_result", "tool_use_id": "b", "is_error": true, "content": [{"type": "text", "text": "Exit code 1"}]}]}}),
                json!({"message": {"content": [{"type": "tool_result", "tool_use_id": "a", "content": "fn x() {}"}]}}),
            ],
        );
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap() + "\n{\"partial\":").unwrap();
        let calls = read_tool_calls(&path).unwrap();
        assert_eq!(calls.iter().map(|c| c.tool.as_str()).collect::<Vec<_>>(), ["Read", "Bash"]);
        assert!(!calls[0].failed && calls[0].result == "fn x() {}");
        assert!(calls[1].failed && calls[1].result == "Exit code 1");
    }

    #[test]
    fn cues_follow_what_the_hook_will_see() {
        let root = Path::new("/r");
        let file = |p: &str| Some(WorkCue::File { path: p.into() });
        assert_eq!(cue_of(&call("Read", json!({"file_path": "/r/src/a.rs"}), false, ""), root), file("src/a.rs"));
        assert_eq!(cue_of(&call("Edit", json!({"file_path": "/r/docs/b.md"}), false, ""), root), file("docs/b.md"));
        assert_eq!(cue_of(&call("Bash", json!({"command": "sed -n 1,9p /r/c.toml"}), false, ""), root), file("c.toml"));
        assert_eq!(cue_of(&call("Read", json!({"file_path": "/elsewhere/a.rs"}), false, ""), root), None, "outside the repo");
        assert_eq!(cue_of(&call("Read", json!({"file_path": "/r/.context/x"}), false, ""), root), None, "skip list");
        assert_eq!(cue_of(&call("Bash", json!({"command": "cargo test"}), false, ""), root), None);
        assert_eq!(cue_of(&call("Bash", json!({"command": "cargo test"}), true, "Exit code 101"), root), Some(WorkCue::Error { text: "Exit code 101".into() }));
        for unhooked in ["<tool_use_error>InputValidationError: x</tool_use_error>", "Permission for this action was denied by the Claude Code auto mode classifier."] {
            assert_eq!(cue_of(&call("Bash", json!({}), true, unhooked), root), None, "{unhooked}");
        }
    }

    #[test]
    fn a_transcript_replays_as_prompts_compactions_and_a_cue_per_finished_call() {
        let dir = tempfile::tempdir().unwrap();
        let root = Path::new("/r");
        let path = write(
            dir.path(),
            "s.jsonl",
            &[
                json!({"type": "user", "permissionMode": "plan", "message": {"content": "tag the release"}}),
                json!({"type": "user", "isMeta": true, "message": {"content": "meta"}}),
                json!({"type": "user", "message": {"content": "<task-notification>x"}}),
                json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "a", "name": "Read", "input": {"file_path": "/r/docs/release.md"}}]}}),
                json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "a", "content": "     1\t# Release checklist\n     2\tbody"}]}}),
                json!({"type": "system", "subtype": "compact_boundary"}),
                json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "cargo test"}}]}}),
                json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "b", "is_error": true, "content": "Exit code 101\npanicked"}]}}),
            ],
        );
        let steps = read_steps(&path, root, None).unwrap();
        let shape: Vec<String> = steps.iter().map(|s| match s {
            ReplayStep::Prompt { prompt, plan } => format!("prompt:{prompt}:{plan}"),
            ReplayStep::Compact => "compact".into(),
            ReplayStep::Cue { cue: WorkCue::File { path }, .. } => format!("file:{path}"),
            ReplayStep::Cue { cue: WorkCue::Error { text }, .. } => format!("error:{}", text.lines().count()),
        }).collect();
        assert_eq!(shape, ["prompt:tag the release:true", "file:docs/release.md", "compact", "error:2"]);
        let sub = read_steps(&path, root, Some("agent-a")).unwrap();
        assert!(sub.iter().all(|s| matches!(s, ReplayStep::Cue { agent_id: Some(_), .. })), "a subagent has no prompts or compactions of its own");
    }

    #[test]
    fn transcripts_cover_main_sessions_and_their_subagents() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(repo.path()).unwrap();
        let dir = home.path().join("projects").join(crate::usage::sanitize_project_path(&canonical));
        std::fs::create_dir_all(dir.join("s1").join("subagents")).unwrap();
        let read = |id: &str| json!({"message": {"content": [{"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": "a.rs"}}]}});
        write(&dir, "s1.jsonl", &[read("m")]);
        write(&dir, "s2.jsonl", &[read("n")]);
        write(&dir.join("s1").join("subagents"), "agent-x.jsonl", &[read("o")]);
        let all = read_transcripts(repo.path(), Some(home.path()), None, None).unwrap();
        assert_eq!(all.iter().map(|t| (t.session_id.as_str(), t.agent_id.as_deref())).collect::<Vec<_>>(), [("s1", None), ("s1", Some("agent-x")), ("s2", None)]);
        let one = read_transcripts(repo.path(), Some(home.path()), None, Some(&["s2".to_string()])).unwrap();
        assert_eq!(one.len(), 1);
    }
}
