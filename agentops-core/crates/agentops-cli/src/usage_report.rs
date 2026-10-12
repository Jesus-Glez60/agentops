//! `agentops usage report` — where a Claude Code session's tokens went,
//! replayed offline from its JSONL transcript (token-reduction plan, step 1).
//!
//! Nearly all of a long session's tokens are cache *re-reads*: every call
//! re-processes the whole context so far. So an item's real cost is its
//! size times the number of calls that carry it, not its size once. This
//! module attributes that cost by category and simulates the session-reset
//! lever, at no plan-quota cost.
//!
//! **Calibration**: the provider-reported context of consecutive calls
//! (`input + cache_read + cache_write`) is exact, so the growth between two
//! calls is split across the items added in between, in proportion to
//! their character counts. Measured on a real session, transcript content
//! runs ~2.3 chars/token, not the 4 a chars/4 estimate assumes. Thinking
//! blocks carry no text in the transcript, so their share lands on the
//! interval's other items.
//!
//! **Segments**: an auto-compaction (`system`/`compact_boundary`, or a >50%
//! context drop as fallback) drops earlier items, so they stop accruing.
//! Back-tested 2026-10-09: all 3 real markers in the local corpus coincided
//! with a >50% drop, and no other drop occurred once `<synthetic>`
//! zero-usage calls are skipped.

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use agentops_graph::{NewSessionReport, ResetSim};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

/// Fallback chars-per-token for an interval whose context didn't grow
/// (cache quirks) — the median measured over a real session.
const CHARS_PER_TOKEN: f64 = 2.3;
/// Character-equivalent charged for an image block, whose base64 isn't
/// in the transcript (~1.5k tokens at `CHARS_PER_TOKEN`).
const IMAGE_CHARS: usize = 3_500;

#[derive(Debug, Default)]
struct Transcript {
    /// Provider-reported context per real API call, in order.
    contexts: Vec<i64>,
    output_tokens: i64,
    /// `(interval, category, chars)`: added before call `interval`.
    items: Vec<(usize, String, usize)>,
    /// Call indexes that start a new segment (after a compaction).
    breaks: Vec<usize>,
    tasks: usize,
}

fn text_len(content: &Value) -> usize {
    match content {
        Value::String(s) => s.len(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|b| match b.get("type").and_then(Value::as_str) {
                Some("image") => IMAGE_CHARS,
                _ => b.get("text").and_then(Value::as_str).map_or(0, str::len),
            })
            .sum(),
        _ => 0,
    }
}

/// `mcp__agentops__search_docs` -> `mcp:search_docs`; built-ins as-is.
fn tool_category(name: &str) -> String {
    match name.strip_prefix("mcp__") {
        Some(rest) => format!("mcp:{}", rest.split_once("__").map_or(rest, |(_, tool)| tool)),
        None => format!("tool:{name}"),
    }
}

fn parse_transcript(path: &Path) -> Result<Transcript> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut t = Transcript::default();
    let mut call_of_message: HashMap<String, usize> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();

    for line in std::io::BufReader::new(file).lines() {
        // A live session's trailing line can be partial -- skip, don't fail.
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let interval = t.contexts.len();
        let message = v.get("message");
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let Some(m) = message else { continue };
                if m.get("model").and_then(Value::as_str) == Some("<synthetic>") {
                    continue;
                }
                let u = m.get("usage");
                let get = |k: &str| u.and_then(|u| u.get(k)).and_then(Value::as_i64).unwrap_or(0);
                let context = get("input_tokens") + get("cache_read_input_tokens") + get("cache_creation_input_tokens");
                // One line per content block, each repeating the message's
                // usage (gotcha 54236): one call per message id.
                let id = m.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                if context > 0 && (id.is_empty() || !call_of_message.contains_key(&id)) {
                    call_of_message.insert(id, t.contexts.len());
                    t.contexts.push(context);
                    t.output_tokens += get("output_tokens");
                }
                for block in m.get("content").and_then(Value::as_array).into_iter().flatten() {
                    let interval = t.contexts.len();
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("unknown");
                            if let Some(id) = block.get("id").and_then(Value::as_str) {
                                tool_names.insert(id.to_string(), name.to_string());
                            }
                            let chars = block.get("input").map_or(0, |i| i.to_string().len());
                            t.items.push((interval, format!("own:{name}"), chars));
                        }
                        Some("text") => t.items.push((interval, "own:text".into(), text_len(&Value::Array(vec![block.clone()])))),
                        _ => {}
                    }
                }
            }
            Some("user") => {
                let Some(content) = message.and_then(|m| m.get("content")) else { continue };
                let meta = v.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
                if v.get("isCompactSummary").and_then(Value::as_bool).unwrap_or(false) {
                    t.items.push((interval, "compaction-summary".into(), text_len(content)));
                    continue;
                }
                match content {
                    Value::Array(blocks) => {
                        for b in blocks {
                            if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                                let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or_default();
                                let category = tool_names.get(id).map_or_else(|| "tool:unknown".to_string(), |n| tool_category(n));
                                t.items.push((interval, category, b.get("content").map_or(0, text_len)));
                            } else {
                                t.items.push((interval, if meta { "reminders" } else { "user-prompt" }.into(), text_len(&Value::Array(vec![b.clone()]))));
                            }
                        }
                    }
                    other => {
                        let is_task = !meta && other.as_str().is_some_and(|s| !s.starts_with('<') && !s.starts_with("[Request interrupted"));
                        t.tasks += usize::from(is_task);
                        t.items.push((interval, if is_task { "user-prompt" } else { "reminders" }.into(), text_len(other)));
                    }
                }
            }
            Some("attachment") => {
                t.items.push((interval, "reminders".into(), v.get("attachment").map_or(0, |a| a.to_string().len())));
            }
            Some("system") if v.get("subtype").and_then(Value::as_str) == Some("compact_boundary") => t.breaks.push(interval),
            _ => {}
        }
    }

    // Fallback break: a >50% drop with no marker (older transcripts).
    for i in 1..t.contexts.len() {
        if t.contexts[i] * 2 < t.contexts[i - 1] && !t.breaks.contains(&i) {
            t.breaks.push(i);
        }
    }
    t.breaks.sort_unstable();
    Ok(t)
}

#[derive(Debug, Serialize)]
pub struct SessionReport {
    pub session: String,
    pub calls: usize,
    pub tasks: usize,
    pub baseline_tokens: i64,
    pub peak_context: i64,
    pub total_context_tokens: i64,
    pub output_tokens: i64,
    pub compactions: usize,
    /// Re-read cost per category: calibrated tokens x calls carrying them.
    pub categories: BTreeMap<String, i64>,
    pub resets: Vec<ResetSim>,
}

impl SessionReport {
    /// The store/wire form, always `source: "transcript"` -- this module
    /// only ever reads real transcripts.
    pub fn to_new(&self) -> NewSessionReport {
        NewSessionReport {
            repo: String::new(),
            session_id: self.session.clone(),
            source: "transcript".into(),
            agent: "claude-code".into(),
            calls: self.calls as i64,
            tasks: self.tasks as i64,
            baseline_tokens: self.baseline_tokens,
            peak_context: self.peak_context,
            total_context_tokens: self.total_context_tokens,
            output_tokens: self.output_tokens,
            compactions: self.compactions as i64,
            categories: self.categories.clone(),
            resets: self.resets.clone(),
        }
    }

    fn per_task(&self) -> Option<i64> {
        (self.tasks > 0).then(|| self.total_context_tokens / self.tasks as i64)
    }
}

/// Replays `contexts` with a simulated reset to `baseline + handoff`
/// whenever context exceeds `threshold`; later calls carry forward only
/// what they added after it. A real compaction (`breaks`) re-bases too.
pub(crate) fn simulate_reset(contexts: &[i64], breaks: &[usize], baseline: i64, threshold: i64, handoff: i64) -> ResetSim {
    let actual: i64 = contexts.iter().sum();
    let (mut offset, mut simulated, mut resets) = (0i64, 0i64, 0usize);
    for (i, &c) in contexts.iter().enumerate() {
        if breaks.contains(&i) {
            offset = 0;
        }
        let mut sim = c - offset;
        if sim > threshold {
            offset = c - (baseline + handoff);
            sim = baseline + handoff;
            resets += 1;
        }
        simulated += sim.max(baseline.min(c));
    }
    let savings = if actual > 0 { 1.0 - simulated as f64 / actual as f64 } else { 0.0 };
    ResetSim { threshold, resets, simulated_tokens: simulated, savings }
}

fn build_report(session: String, t: &Transcript, thresholds: &[i64], handoff: i64) -> SessionReport {
    let n = t.contexts.len();
    let baseline = t.contexts.first().copied().unwrap_or(0);
    let mut categories: BTreeMap<String, i64> = BTreeMap::new();
    *categories.entry("baseline".into()).or_default() = baseline * n as i64;

    let mut by_interval: BTreeMap<usize, Vec<&(usize, String, usize)>> = BTreeMap::new();
    for item in &t.items {
        // Before the first call it's baseline; after the last, never read.
        if item.0 >= 1 && item.0 < n {
            by_interval.entry(item.0).or_default().push(item);
        }
    }
    for (&i, items) in &by_interval {
        let prev = if t.breaks.contains(&i) { baseline } else { t.contexts[i - 1] };
        let delta = t.contexts[i] - prev;
        let chars: usize = items.iter().map(|it| it.2).sum();
        let segment_end = t.breaks.iter().copied().find(|&b| b > i).unwrap_or(n);
        let carried = (segment_end - i) as i64;
        for (_, category, c) in items {
            let tokens = if delta > 0 && chars > 0 { delta as f64 * *c as f64 / chars as f64 } else { *c as f64 / CHARS_PER_TOKEN };
            *categories.entry(category.clone()).or_default() += (tokens * carried as f64) as i64;
        }
        if chars == 0 && delta > 0 {
            *categories.entry("unattributed".into()).or_default() += delta * carried;
        }
    }

    SessionReport {
        session,
        calls: n,
        tasks: t.tasks,
        baseline_tokens: baseline,
        peak_context: t.contexts.iter().copied().max().unwrap_or(0),
        total_context_tokens: t.contexts.iter().sum(),
        output_tokens: t.output_tokens,
        compactions: t.breaks.len(),
        categories,
        resets: thresholds.iter().map(|&th| simulate_reset(&t.contexts, &t.breaks, baseline, th, handoff)).collect(),
    }
}

/// `100k`/`1.5m`/`150000` -> tokens.
pub fn parse_tokens(s: &str) -> Result<i64> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = match s.strip_suffix('k') {
        Some(n) => (n, 1_000.0),
        None => s.strip_suffix('m').map_or((s.as_str(), 1.0), |n| (n, 1_000_000.0)),
    };
    Ok((num.parse::<f64>().with_context(|| format!("not a token count: {s:?}"))? * mult) as i64)
}

/// Every transcript for the repo: `(label, path)`, newest first; subagent
/// transcripts are labelled `<session>/<file stem>`.
fn transcripts(projects_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(projects_dir).with_context(|| format!("reading {}", projects_dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let session = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();
        if let Ok(subs) = std::fs::read_dir(projects_dir.join(&session).join("subagents")) {
            for sub in subs.flatten().map(|s| s.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl")) {
                let stem = sub.file_stem().and_then(|s| s.to_str()).unwrap_or("agent").to_string();
                found.push((format!("{session}/{stem}"), sub));
            }
        }
        found.push((session, path));
    }
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    found.sort_by_key(|(_, p)| std::cmp::Reverse(mtime(p)));
    Ok(found)
}

fn fmt_tokens(n: i64) -> String {
    match n.abs() {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{}k", n / 1_000),
        _ => n.to_string(),
    }
}

fn print_session(r: &SessionReport, detailed: bool) {
    let mean = if r.calls > 0 { r.total_context_tokens / r.calls as i64 } else { 0 };
    let per_task = r.per_task().map_or("-".into(), fmt_tokens);
    let resets: Vec<String> = r.resets.iter().map(|s| format!("@{}:{:.0}%", fmt_tokens(s.threshold), s.savings * 100.0)).collect();
    println!(
        "{} calls={} tasks={} ctx={}→{} mean={} total={} per-task={} compactions={} reset-sim {}",
        r.session,
        r.calls,
        r.tasks,
        fmt_tokens(r.baseline_tokens),
        fmt_tokens(r.peak_context),
        fmt_tokens(mean),
        fmt_tokens(r.total_context_tokens),
        per_task,
        r.compactions,
        resets.join(" ")
    );
    let mut cats: Vec<_> = r.categories.iter().collect();
    cats.sort_by_key(|(_, v)| std::cmp::Reverse(**v));
    let total = r.total_context_tokens.max(1) as f64;
    let shown = if detailed { cats.len() } else { 3 };
    let line: Vec<String> = cats.iter().take(shown).map(|(k, v)| format!("{k} {} ({:.0}%)", fmt_tokens(**v), **v as f64 / total * 100.0)).collect();
    println!("  {}", line.join(" | "));
    if detailed {
        for s in &r.resets {
            println!("  reset at {}: {} resets, {} → {} ({:.0}% saved, upper bound)", fmt_tokens(s.threshold), s.resets, fmt_tokens(r.total_context_tokens), fmt_tokens(s.simulated_tokens), s.savings * 100.0);
        }
    }
}

const DEFAULT_THRESHOLDS: [i64; 3] = [100_000, 150_000, 200_000];
const DEFAULT_HANDOFF: i64 = 30_000;

/// Reports for `usage sync` to store or push: every transcript of the repo,
/// or -- with `transcript` (the SessionEnd hook's path) -- just that session
/// and its subagents. Default thresholds/handoff, matching `usage report`.
pub fn reports_for_sync(repo_path: &Path, claude_home: Option<&Path>, transcript: Option<&Path>) -> Result<Vec<NewSessionReport>> {
    let files: Vec<(String, PathBuf)> = match transcript {
        Some(file) => {
            let session = file.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string();
            let mut files = vec![(session.clone(), file.to_path_buf())];
            if let Ok(subs) = std::fs::read_dir(file.with_extension("").join("subagents")) {
                for sub in subs.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl")) {
                    let stem = sub.file_stem().and_then(|s| s.to_str()).unwrap_or("agent").to_string();
                    files.push((format!("{session}/{stem}"), sub));
                }
            }
            files
        }
        None => {
            let canonical = repo_path.canonicalize().with_context(|| format!("canonicalizing {}", repo_path.display()))?;
            let claude_home = claude_home.map(PathBuf::from).unwrap_or_else(crate::usage::default_claude_home);
            let projects_dir = claude_home.join("projects").join(crate::usage::sanitize_project_path(&canonical));
            if !projects_dir.exists() {
                return Ok(Vec::new());
            }
            transcripts(&projects_dir)?
        }
    };
    let mut reports = Vec::new();
    for (label, path) in files {
        let t = parse_transcript(&path)?;
        if !t.contexts.is_empty() {
            reports.push(build_report(label, &t, &DEFAULT_THRESHOLDS, DEFAULT_HANDOFF).to_new());
        }
    }
    Ok(reports)
}

pub struct ReportArgs<'a> {
    pub path: &'a Path,
    pub claude_home: Option<&'a Path>,
    pub session: Option<&'a str>,
    pub all: bool,
    pub thresholds: Vec<i64>,
    pub handoff: i64,
    pub json: bool,
}

pub fn report_command(args: ReportArgs) -> Result<()> {
    let canonical = args.path.canonicalize().with_context(|| format!("canonicalizing {}", args.path.display()))?;
    let claude_home = args.claude_home.map(PathBuf::from).unwrap_or_else(crate::usage::default_claude_home);
    let projects_dir = claude_home.join("projects").join(crate::usage::sanitize_project_path(&canonical));
    let all = transcripts(&projects_dir)?;

    let selected: Vec<&(String, PathBuf)> = match (args.session, args.all) {
        (_, true) => all.iter().collect(),
        (Some(id), false) => all.iter().filter(|(label, _)| label == id || label.starts_with(&format!("{id}/"))).collect(),
        (None, false) => all.iter().filter(|(label, _)| !label.contains('/')).take(1).collect(),
    };
    anyhow::ensure!(!selected.is_empty(), "no matching transcripts under {}", projects_dir.display());

    let mut reports = Vec::new();
    for (label, path) in selected {
        let t = parse_transcript(path)?;
        if !t.contexts.is_empty() {
            reports.push(build_report(label.clone(), &t, &args.thresholds, args.handoff));
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }
    let detailed = reports.len() == 1;
    for r in &reports {
        print_session(r, detailed);
    }
    if reports.len() > 1 {
        let actual: i64 = reports.iter().map(|r| r.total_context_tokens).sum();
        let sims: Vec<String> = args
            .thresholds
            .iter()
            .enumerate()
            .map(|(i, th)| {
                let sim: i64 = reports.iter().map(|r| r.resets[i].simulated_tokens).sum();
                format!("@{}:{:.0}%", fmt_tokens(*th), (1.0 - sim as f64 / actual.max(1) as f64) * 100.0)
            })
            .collect();
        println!("TOTAL {} transcripts, {} context tokens, reset-sim {} (upper bound: re-exploration after a reset isn't modelled)", reports.len(), fmt_tokens(actual), sims.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(lines: &[&str]) -> Transcript {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, lines.join("\n")).unwrap();
        parse_transcript(&path).unwrap()
    }

    fn call(id: &str, context: i64, content: &str) -> String {
        format!(r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":0,"cache_read_input_tokens":{context},"cache_creation_input_tokens":0,"output_tokens":5}},"content":[{content}]}}}}"#)
    }

    fn tool_result(id: &str, content: &str) -> String {
        format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":{content}}}]}}}}"#)
    }

    #[test]
    fn dedupes_repeated_message_ids_and_skips_synthetic_calls() {
        let t = transcript(&[
            &call("m1", 100, r#"{"type":"thinking","thinking":""}"#),
            &call("m1", 100, r#"{"type":"text","text":"hi"}"#),
            r#"{"type":"assistant","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":0},"content":[]}}"#,
            &call("m2", 150, "{}"),
        ]);
        assert_eq!(t.contexts, vec![100, 150]);
        assert_eq!(t.output_tokens, 10);
    }

    #[test]
    fn splits_a_context_delta_across_results_by_size_for_every_content_shape() {
        let t = transcript(&[
            &call("m1", 1_000, r#"{"type":"tool_use","id":"a","name":"Bash","input":{}},{"type":"tool_use","id":"b","name":"mcp__agentops__search_docs","input":{}},{"type":"tool_use","id":"c","name":"Read","input":{}}"#),
            // Gotcha 53972: bare string, text blocks, and an image block.
            &tool_result("a", &format!("\"{}\"", "x".repeat(3_000))),
            &tool_result("b", &format!(r#"[{{"type":"text","text":"{}"}}]"#, "y".repeat(1_000))),
            &tool_result("c", r#"[{"type":"image","source":{}}]"#),
            &call("m2", 1_000 + 7_500, "{}"),
            &call("m3", 8_500, "{}"),
        ]);
        let r = build_report("s".into(), &t, &[], 0);
        // 7,500 tokens over 3,000 + 1,000 + 3,500 chars, carried by 2 calls
        // (m2, m3). Own tool_use inputs ("{}", 2 chars each) are in the
        // same interval and take a sliver.
        let chars = 3_000.0 + 1_000.0 + IMAGE_CHARS as f64 + 6.0;
        let expect = |c: f64| (7_500.0 * c / chars * 2.0) as i64;
        assert!((r.categories["tool:Bash"] - expect(3_000.0)).abs() <= 2);
        assert!((r.categories["mcp:search_docs"] - expect(1_000.0)).abs() <= 2);
        assert!((r.categories["tool:Read"] - expect(IMAGE_CHARS as f64)).abs() <= 2);
        assert_eq!(r.categories["baseline"], 1_000 * 3);
    }

    #[test]
    fn a_compaction_marker_ends_the_segment_and_rebases_on_baseline() {
        let t = transcript(&[
            &call("m1", 1_000, "{}"),
            r#"{"type":"user","message":{"content":"a real question"}}"#,
            &call("m2", 3_000, "{}"),
            &call("m3", 3_000, "{}"),
            r#"{"type":"system","subtype":"compact_boundary"}"#,
            r#"{"type":"user","isCompactSummary":true,"message":{"content":"summary"}}"#,
            &call("m4", 1_500, "{}"),
        ]);
        assert_eq!(t.breaks, vec![3]);
        assert_eq!(t.tasks, 1);
        let r = build_report("s".into(), &t, &[], 0);
        // The prompt (2,000 tokens) is carried by m2, m3 only, not m4.
        assert_eq!(r.categories["user-prompt"], 4_000);
        // The summary's interval is measured from baseline: 500 x 1 call.
        assert_eq!(r.categories["compaction-summary"], 500);
    }

    #[test]
    fn reset_simulation_rebases_after_each_of_several_resets() {
        // baseline 10, handoff 5, threshold 50, +20 per call.
        let contexts = vec![10, 30, 50, 70, 90, 110, 130];
        let s = simulate_reset(&contexts, &[], 10, 50, 5);
        // 10 30 50 | 70>50 -> 15 | 90-55=35 | 110-55=55>50 -> 15 | 130-95=35
        assert_eq!(s.resets, 2);
        assert_eq!(s.simulated_tokens, 10 + 30 + 50 + 15 + 35 + 15 + 35);
        let actual: i64 = contexts.iter().sum();
        assert!((s.savings - (1.0 - 190.0 / actual as f64)).abs() < 1e-9);
    }

    #[test]
    fn reset_simulation_rebases_on_a_real_compaction() {
        let s = simulate_reset(&[10, 60, 70, 20, 40], &[3], 10, 50, 5);
        // 10 | 60>50 -> 15 | 70-45=25 | compaction: offset 0 -> 20 | 40
        assert_eq!((s.resets, s.simulated_tokens), (1, 10 + 15 + 25 + 20 + 40));
    }

    #[test]
    fn parses_token_counts_with_suffixes() {
        assert_eq!(parse_tokens("150k").unwrap(), 150_000);
        assert_eq!(parse_tokens("1.5M").unwrap(), 1_500_000);
        assert_eq!(parse_tokens("2000").unwrap(), 2_000);
    }
}
