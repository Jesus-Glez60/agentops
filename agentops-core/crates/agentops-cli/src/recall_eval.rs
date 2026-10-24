//! `agentops recall-eval`: scores the prompt-time recall seed against a
//! fixed set of cases (milestone 3a, item 0). Every case goes through the
//! code path a real `UserPromptSubmit` hook takes, `hooks::handle` locally
//! or `POST /repos/{id}/hooks/userpromptsubmit` with `--remote`, and is
//! scored on what the hook actually injected, after its budget cut.
//!
//! Cases are JSONL, one `{id, prompt, mode, targets}` per line, kept under
//! `.context/` (git-ignored: prompts can reveal private memories). A run
//! writes per-case and overall results as JSON under `.context/recall-eval/`
//! and prints a table without prompt text, so only numbers leave the box.
//!
//! A recalled memory whose stored vector has cosine ≥ `EQUIVALENT_MIN_SCORE`
//! with a target counts as an "equivalent" hit: the same fact recorded
//! twice. `SimilarTo` edges keep no score, so the cosine is read from the
//! stored vectors (the server's `GET /repos/{id}/nodes/{node_id}/similarity`
//! remotely). No embedding model is loaded either way.
//!
//! Eval sessions are recorded like real ones, under session ids starting
//! `recall-eval-`; per-session statistics should filter that prefix out.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use agentops_graph::{GraphStore, NodeKind, NodeProminence};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::remote_marker::{read_remote_marker, RemoteMcpMarker};

/// Cosine at or above which a recalled memory holds the same fact as a
/// target (milestone 2 found same-fact pairs in the 0.85-0.90 band).
pub(crate) const EQUIVALENT_MIN_SCORE: f32 = 0.88;

/// The header `hooks::push_seed` puts above the recall pointers. Pins come
/// before it in the same context and use the same `- [kind]` line shape,
/// so parsing starts after it.
const SEED_HEADER: &str = "Recalled from AgentOps for this task";

/// Kinds an equivalent may be: memories, never generated doc sections.
const MEMORY_KINDS: &[NodeKind] = &[NodeKind::Gotcha, NodeKind::Decision, NodeKind::Note];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    Plan,
    Default,
}

impl Mode {
    fn permission_mode(self) -> &'static str {
        match self {
            Mode::Plan => "plan",
            Mode::Default => "default",
        }
    }
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    prompt: String,
    mode: Mode,
    targets: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum HitType {
    Exact,
    Equivalent,
    Miss,
}

#[derive(Debug, Serialize)]
struct CaseResult {
    case_id: String,
    mode: Mode,
    first_hit_rank: Option<usize>,
    hit_type: HitType,
    digest_in_seed: bool,
    seed_kinds: Vec<String>,
    /// Median of this case's runs.
    latency_ms: f64,
    latency_runs_ms: Vec<f64>,
    /// Whether every repeat injected the same seed.
    seed_stable: bool,
    /// Rank of the first target or equivalent in the deep ranking (up to
    /// `DEEP_LIMIT` pointers from the same retrieval, before the seed's
    /// cut), or `None` when recall never reaches it.
    deep_rank: Option<usize>,
    /// Why a miss missed (see `miss_category`); `None` for a hit.
    miss_category: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct Summary {
    cases: usize,
    runs: usize,
    hit_rate: f64,
    exact: usize,
    equivalent: usize,
    mrr: f64,
    p50_ms: f64,
    p95_ms: f64,
    /// Misses per `miss_category`.
    misses: std::collections::BTreeMap<&'static str, usize>,
}

/// How deep `recall-eval` looks past the seed: the server's
/// `POST /repos/{id}/recall` cap.
pub(crate) const DEEP_LIMIT: usize = 50;

/// One pointer of the deep ranking.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DeepPointer {
    pub(crate) node_id: i64,
    pub(crate) kind: String,
    pub(crate) reason: String,
}

/// Where the hook runs: this repo's local graph, or the server it's
/// connected to. Locally, `embedder` adds recall by meaning exactly as
/// the server's hook route does (`--embed`); a real local hook never
/// loads the model (ADR 0052).
pub(crate) enum Backend {
    Local { store: Box<dyn GraphStore>, repo: String, embedder: Option<&'static (dyn agentops_embeddings::Embedder + Sync)> },
    Remote(RemoteMcpMarker),
}

/// `--embed`'s embedder: the model the server warms at startup.
pub(crate) static EVAL_EMBEDDER: agentops_embeddings::LocalEmbedder = agentops_embeddings::LocalEmbedder;

/// The local backend for `path`, with the model when `embed` is set.
pub(crate) fn local_backend(path: &Path, embed: bool) -> Result<Backend> {
    let embedder: Option<&'static (dyn agentops_embeddings::Embedder + Sync)> = if embed { Some(&EVAL_EMBEDDER) } else { None };
    Ok(Backend::Local { store: agentops_mcp::open_store(path)?, repo: agentops_mcp::repo_name(path), embedder })
}

impl Backend {
    /// The hook's output for `payload`, as `hooks::handle` returns it.
    fn prompt(&self, payload: &Value) -> Result<Option<Value>> {
        match self {
            Backend::Local { store, repo, embedder } => agentops_mcp::hooks::handle_with(store.as_ref(), repo, "userpromptsubmit", payload, *embedder),
            Backend::Remote(marker) => crate::hook_capture::call_remote(marker, "userpromptsubmit", payload),
        }
    }

    /// Cosine between `node`'s stored vector and each of `to`'s; nodes
    /// without a vector are left out.
    fn cosines(&self, node: i64, to: &[i64]) -> Result<HashMap<i64, f32>> {
        match self {
            Backend::Local { store, repo, .. } => {
                let mut out = HashMap::new();
                let Some(source) = store.get_embedding(repo, node)? else { return Ok(out) };
                for &other in to {
                    if let Some(vector) = store.get_embedding(repo, other)? {
                        out.insert(other, agentops_graph::similarity::cosine(&source, &vector));
                    }
                }
                Ok(out)
            }
            Backend::Remote(marker) => {
                let ids = to.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
                let reply = remote_get(marker, &format!("nodes/{node}/similarity?to={ids}"))?;
                let scores = reply.get("scores").and_then(Value::as_object).cloned().unwrap_or_default();
                Ok(scores.into_iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_f64()? as f32))).collect())
            }
        }
    }

    /// The deep ranking for `prompt`: the retrieval the hook runs, up to
    /// `DEEP_LIMIT` pointers, with nothing recorded.
    pub(crate) fn deep(&self, prompt: &str) -> Result<Vec<DeepPointer>> {
        match self {
            Backend::Local { store, repo, embedder } => Ok(agentops_retrieval::recall_seed_with(store.as_ref(), repo, prompt, DEEP_LIMIT, &Default::default(), *embedder)?
                .into_iter()
                .map(|p| DeepPointer { node_id: p.node_id, kind: p.kind.as_db_str().to_string(), reason: p.reason })
                .collect()),
            Backend::Remote(marker) => {
                let reply = remote_send(marker, "recall", Some(&json!({ "prompt": prompt, "limit": DEEP_LIMIT })))?;
                serde_json::from_value(reply["pointers"].clone()).context("reading the recall route's pointers")
            }
        }
    }

    /// A node's prominence, or `None` if it doesn't exist.
    pub(crate) fn prominence(&self, node: i64) -> Result<Option<NodeProminence>> {
        match self {
            Backend::Local { store, repo, .. } => Ok(store.get_node(repo, node)?.map(|n| n.prominence)),
            Backend::Remote(marker) => match remote_get(marker, &format!("nodes/{node}")) {
                Ok(detail) => Ok(Some(serde_json::from_value(detail["prominence"].clone()).context("reading a node's prominence")?)),
                Err(e) if e.to_string().contains("returned 404") => Ok(None),
                Err(e) => Err(e),
            },
        }
    }
}

/// `GET {server}/repos/{connection}/{rest}` with the marker's API key.
fn remote_get(marker: &RemoteMcpMarker, rest: &str) -> Result<Value> {
    remote_send(marker, rest, None)
}

/// `GET`, or `POST` with a JSON body, to `{server}/repos/{connection}/{rest}`.
pub(crate) fn remote_send(marker: &RemoteMcpMarker, rest: &str, body: Option<&Value>) -> Result<Value> {
    remote_send_within(marker, rest, body, crate::hook_capture::REMOTE_HOOK_TIMEOUT)
}

/// `remote_send` with its own timeout, for calls slower than a hook (a
/// replay chunk).
pub(crate) fn remote_send_within(marker: &RemoteMcpMarker, rest: &str, body: Option<&Value>, timeout: std::time::Duration) -> Result<Value> {
    let url = format!("{}/repos/{}/{rest}", marker.server_url.trim_end_matches('/'), marker.connection_id);
    let auth = format!("Bearer {}", marker.api_key);
    let method = if body.is_some() { "POST" } else { "GET" };
    let response = match body {
        Some(body) => ureq::post(&url).header("Authorization", &auth).config().http_status_as_error(false).timeout_global(Some(timeout)).build().send_json(body),
        None => ureq::get(&url).header("Authorization", &auth).config().http_status_as_error(false).timeout_global(Some(timeout)).build().call(),
    };
    let mut response = response.with_context(|| format!("calling {method} {url}"))?;
    if !response.status().is_success() {
        bail!("{method} {url} returned {}", response.status().as_u16());
    }
    response.body_mut().read_json().with_context(|| format!("parsing {method} {url} response"))
}

pub(crate) fn run(path: &Path, remote: bool, embed: bool, cases_file: Option<&Path>, mode: Option<Mode>, repeat: usize) -> Result<()> {
    anyhow::ensure!(!(remote && embed), "--embed is for a local graph; the server already embeds");
    let cases_file = cases_file.map(Path::to_path_buf).unwrap_or_else(|| path.join(".context").join("recall-eval.jsonl"));
    let cases = read_cases(&cases_file)?;
    let backend = if remote {
        Backend::Remote(read_remote_marker(path).with_context(|| format!("--remote needs a server connection; {} has no .context/agentops-remote.json", path.display()))?)
    } else {
        local_backend(path, embed)?
    };
    let (results, summary) = evaluate(&backend, &cases, mode, repeat.max(1))?;

    print!("{}", render_table(&results, &summary));
    let out_dir = path.join(".context").join("recall-eval");
    std::fs::create_dir_all(&out_dir)?;
    let out = out_dir.join(format!("{}.json", unix_secs()));
    std::fs::write(&out, serde_json::to_string_pretty(&json!({ "remote": remote, "cases": results, "summary": summary }))?)?;
    println!("Wrote {}", out.display());
    Ok(())
}

fn read_cases(file: &Path) -> Result<Vec<Case>> {
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let cases: Vec<Case> = text.lines().filter(|l| !l.trim().is_empty()).enumerate().map(|(i, l)| serde_json::from_str(l).with_context(|| format!("{} line {}", file.display(), i + 1))).collect::<Result<_>>()?;
    if cases.is_empty() {
        bail!("{} has no cases", file.display());
    }
    Ok(cases)
}

fn evaluate(backend: &Backend, cases: &[Case], mode: Option<Mode>, repeat: usize) -> Result<(Vec<CaseResult>, Summary)> {
    // A target the seed can never show would read as a recall miss.
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
    }

    let run_id = unix_secs();
    let mut equivalent_cache: HashMap<(i64, Vec<i64>), bool> = HashMap::new();
    let mut results = Vec::new();
    let mut all_latencies = Vec::new();
    for case in cases {
        let mode = mode.unwrap_or(case.mode);
        let mut seeds = Vec::new();
        let mut latencies = Vec::new();
        for n in 0..repeat {
            let payload = json!({
                "session_id": format!("recall-eval-{run_id}-{}-{n}", case.id),
                "hook_event_name": "UserPromptSubmit",
                "prompt": case.prompt,
                "permission_mode": mode.permission_mode(),
            });
            let started = Instant::now();
            let output = backend.prompt(&payload)?;
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
            seeds.push(parse_seed(output.as_ref()));
        }
        let seed = &seeds[0];
        let mut first_hit = None;
        for (i, (id, kind)) in seed.iter().enumerate() {
            if case.targets.contains(id) {
                first_hit = Some((i + 1, HitType::Exact));
                break;
            }
            if !MEMORY_KINDS.contains(&NodeKind::from_db_str(kind)) {
                continue;
            }
            let key = (*id, case.targets.clone());
            let equivalent = match equivalent_cache.get(&key) {
                Some(&e) => e,
                None => {
                    let e = backend.cosines(*id, &case.targets)?.values().any(|&s| s >= EQUIVALENT_MIN_SCORE);
                    equivalent_cache.insert(key, e);
                    e
                }
            };
            if equivalent {
                first_hit = Some((i + 1, HitType::Equivalent));
                break;
            }
        }
        let deep = backend.deep(&case.prompt)?;
        let deep_rank = deep_rank(backend, &deep, &case.targets)?;
        let miss_category = if first_hit.is_some() { None } else { Some(miss_category(&deep, deep_rank)) };
        all_latencies.extend(&latencies);
        results.push(CaseResult {
            case_id: case.id.clone(),
            mode,
            first_hit_rank: first_hit.map(|(r, _)| r),
            hit_type: first_hit.map_or(HitType::Miss, |(_, t)| t),
            digest_in_seed: seed.iter().any(|(_, k)| k == NodeKind::DocSection.as_db_str()),
            seed_kinds: seed.iter().map(|(_, k)| k.clone()).collect(),
            latency_ms: percentile(&latencies, 50.0),
            latency_runs_ms: latencies,
            seed_stable: seeds.iter().all(|s| s == seed),
            deep_rank,
            miss_category,
        });
    }

    let hits = results.iter().filter(|r| r.hit_type != HitType::Miss).count();
    let summary = Summary {
        cases: results.len(),
        runs: all_latencies.len(),
        hit_rate: hits as f64 / results.len() as f64,
        exact: results.iter().filter(|r| r.hit_type == HitType::Exact).count(),
        equivalent: results.iter().filter(|r| r.hit_type == HitType::Equivalent).count(),
        mrr: results.iter().map(|r| r.first_hit_rank.map_or(0.0, |rank| 1.0 / rank as f64)).sum::<f64>() / results.len() as f64,
        p50_ms: percentile(&all_latencies, 50.0),
        p95_ms: percentile(&all_latencies, 95.0),
        misses: results.iter().filter_map(|r| r.miss_category).fold(Default::default(), |mut m, c| {
            *m.entry(c).or_insert(0) += 1;
            m
        }),
    };
    Ok((results, summary))
}

/// Rank of the first target, or memory equivalent to one, in `deep`. One
/// similarity lookup per target covers the whole list.
fn deep_rank(backend: &Backend, deep: &[DeepPointer], targets: &[i64]) -> Result<Option<usize>> {
    let memories: Vec<i64> = deep.iter().filter(|p| MEMORY_KINDS.contains(&NodeKind::from_db_str(&p.kind))).map(|p| p.node_id).collect();
    let mut equivalent = std::collections::HashSet::new();
    if !memories.is_empty() {
        for &target in targets {
            equivalent.extend(backend.cosines(target, &memories)?.into_iter().filter(|&(_, s)| s >= EQUIVALENT_MIN_SCORE).map(|(id, _)| id));
        }
    }
    Ok(deep.iter().position(|p| targets.contains(&p.node_id) || equivalent.contains(&p.node_id)).map(|i| i + 1))
}

/// Why a case missed:
/// - `vocabulary`: recall never reaches the target (not in the deep
///   ranking): its words don't meet the prompt's.
/// - `noise`: the target ranks below the seed, and a doc section or a
///   `SimilarTo` neighbour holds one of the seed slots above it.
/// - `ranking`: it ranks below the seed behind direct matches only.
///
/// A target that isn't recallable at all (the spec's "missing memory") is
/// refused before any case runs, so it never shows up here.
fn miss_category(deep: &[DeepPointer], deep_rank: Option<usize>) -> &'static str {
    let Some(rank) = deep_rank else { return "vocabulary" };
    let above = &deep[..(rank - 1).min(SEED_SLOTS)];
    if above.iter().any(|p| p.kind == NodeKind::DocSection.as_db_str() || p.reason.starts_with("similar to")) {
        "noise"
    } else {
        "ranking"
    }
}

/// Seed slots in plan mode (`agentops_mcp::budget::PLAN_SEED_MAX_ITEMS`).
const SEED_SLOTS: usize = 8;

/// The `(node id, kind)` of each recall pointer the hook injected, in
/// order. Only lines after `SEED_HEADER` count: pins above it share the
/// line shape.
fn parse_seed(output: Option<&Value>) -> Vec<(i64, String)> {
    let context = output.and_then(|o| o.pointer("/hookSpecificOutput/additionalContext")).and_then(Value::as_str).unwrap_or_default();
    let Some((_, after)) = context.split_once(SEED_HEADER) else { return Vec::new() };
    after.lines().skip(1).take_while(|l| l.starts_with("- [")).filter_map(parse_pointer).collect()
}

/// `- [kind] Title (id N) — reason` → `(N, kind)`. The id is the first
/// `(id <digits>) — ` on the line: a title may contain "(id 3)", but not
/// followed by the em-dash separator `RecallPointer::render` writes.
fn parse_pointer(line: &str) -> Option<(i64, String)> {
    let kind = line.strip_prefix("- [")?.split_once(']')?.0.to_string();
    let mut rest = line;
    while let Some(at) = rest.find("(id ") {
        let tail = &rest[at + 4..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && tail[digits.len()..].starts_with(") — ") {
            return Some((digits.parse().ok()?, kind));
        }
        rest = tail;
    }
    None
}

/// Nearest-rank percentile: the smallest sample with at least `p`% of
/// samples at or below it.
pub(crate) fn percentile(samples: &[f64], p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank - 1]
}

fn render_table(results: &[CaseResult], summary: &Summary) -> String {
    let mut out = format!("{:<10} {:<8} {:>4} {:<11} {:>4} {:<11} {:<6} {:>8}  {}\n", "case", "mode", "rank", "hit", "deep", "miss", "digest", "p50 ms", "seed kinds");
    for r in results {
        let rank = r.first_hit_rank.map_or("-".to_string(), |n| n.to_string());
        let hit = format!("{:?}", r.hit_type).to_lowercase();
        let stable = if r.seed_stable { "" } else { " (seed varied across runs)" };
        let deep = r.deep_rank.map_or("-".to_string(), |n| n.to_string());
        out.push_str(&format!("{:<10} {:<8} {:>4} {:<11} {:>4} {:<11} {:<6} {:>8.0}  {}{stable}\n", r.case_id, r.mode.permission_mode(), rank, hit, deep, r.miss_category.unwrap_or(""), if r.digest_in_seed { "yes" } else { "no" }, r.latency_ms, r.seed_kinds.join(",")));
    }
    out.push_str(&format!(
        "\nhit rate {}/{} ({:.0}%: {} exact, {} equivalent), MRR {:.3}, latency p50 {:.0} ms / p95 {:.0} ms over {} runs\n",
        summary.exact + summary.equivalent,
        summary.cases,
        summary.hit_rate * 100.0,
        summary.exact,
        summary.equivalent,
        summary.mrr,
        summary.p50_ms,
        summary.p95_ms,
        summary.runs
    ));
    if !summary.misses.is_empty() {
        out.push_str(&format!("misses: {}\n", summary.misses.iter().map(|(c, n)| format!("{n} {c}")).collect::<Vec<_>>().join(", ")));
    }
    out
}

pub(crate) fn unix_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{upsert_node, NewNode, SqliteGraphStore};

    fn note(store: &dyn GraphStore, kind: NodeKind, name: &str, content: &str) -> i64 {
        upsert_node(store, NewNode { kind, repo: "r".into(), path: Some(format!("vault:{name}")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap()
    }

    /// A full-size vector whose first two components are `x`, `y`.
    fn vector(x: f32, y: f32) -> Vec<f32> {
        let mut v = vec![0.0; agentops_embeddings::EMBEDDING_DIM];
        (v[0], v[1]) = (x, y);
        v
    }

    fn case(id: &str, prompt: &str, targets: Vec<i64>) -> Case {
        Case { id: id.into(), prompt: prompt.into(), mode: Mode::Plan, targets }
    }

    #[test]
    fn a_pointer_line_yields_its_id_and_kind_even_with_an_id_lookalike_in_the_title() {
        assert_eq!(parse_pointer("- [gotcha] Webhook 401 (id 42) — matched \"webhook\""), Some((42, "gotcha".into())));
        assert_eq!(parse_pointer("- [note] Fix (id 3) first (id 77) — similar to \"A (id 9) — b\""), Some((77, "note".into())));
        assert_eq!(parse_pointer("no pointer here"), None);
    }

    #[test]
    fn only_lines_under_the_seed_header_count_not_the_pins_above_it() {
        let context = "## Pinned\n- [decision] A rule (id 1) — pinned\n\nRecalled from AgentOps for this task (pointers; ...):\n- [gotcha] G (id 5) — matched \"x\"\n- [doc_section] Notes (id 6) — matched \"x\"\n\ntrailing";
        let output = json!({ "hookSpecificOutput": { "additionalContext": context } });
        assert_eq!(parse_seed(Some(&output)), vec![(5, "gotcha".to_string()), (6, "doc_section".to_string())]);
        assert!(parse_seed(None).is_empty());
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&samples, 50.0), 50.0);
        assert_eq!(percentile(&samples, 95.0), 95.0);
        assert_eq!(percentile(&[7.0, 1.0, 3.0], 50.0), 3.0);
        assert_eq!(percentile(&[7.0, 1.0, 3.0], 95.0), 7.0);
        assert_eq!(percentile(&[], 95.0), 0.0);
    }

    #[test]
    fn scores_exact_equivalent_and_miss_and_a_target_without_a_vector_is_just_a_miss() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let exact = note(&store, NodeKind::Gotcha, "Webhook signature fails", "webhook signature 401");
        let twin = note(&store, NodeKind::Decision, "Deploy box rollback", "rollback deploy box");
        let original = note(&store, NodeKind::Note, "Original record", "unrelated words entirely");
        let digest = note(&store, NodeKind::DocSection, "Known Gotchas", "rollback deploy box everything");
        let unembedded = note(&store, NodeKind::Note, "No vector", "nothing");
        store.set_embedding("r", twin, &vector(1.0, 0.0)).unwrap();
        store.set_embedding("r", original, &vector(0.95, 0.31)).unwrap(); // cosine ≈ 0.95
        store.set_embedding("r", digest, &vector(1.0, 0.0)).unwrap();
        let backend = Backend::Local { store: Box::new(store), repo: "r".into(), embedder: None };

        let cases = vec![case("exact", "webhook signature", vec![exact]), case("twin", "rollback deploy box", vec![original]), case("miss", "rollback deploy box", vec![unembedded])];
        let (results, summary) = evaluate(&backend, &cases, None, 2).unwrap();
        assert_eq!((results[0].hit_type, results[0].first_hit_rank), (HitType::Exact, Some(1)), "{results:?}");
        assert_eq!(results[1].hit_type, HitType::Equivalent, "a same-fact memory counts, the digest never does: {results:?}");
        assert!(!results[1].digest_in_seed, "doc sections never reach a seed (milestone 3a item 2)");
        assert_eq!(results[2].hit_type, HitType::Miss, "{results:?}");
        assert_eq!((summary.cases, summary.runs, summary.exact, summary.equivalent), (3, 6, 1, 1));
        assert!((summary.hit_rate - 2.0 / 3.0).abs() < 1e-9);
        assert!(results.iter().all(|r| r.seed_stable && r.latency_runs_ms.len() == 2));
    }

    #[test]
    fn a_miss_is_vocabulary_when_recall_never_reaches_it_noise_behind_a_digest_or_hop_else_ranking() {
        let p = |id: i64, kind: &str, reason: &str| DeepPointer { node_id: id, kind: kind.into(), reason: reason.into() };
        let direct: Vec<DeepPointer> = (1..=9).map(|i| p(i, "gotcha", "matched \"x\"")).collect();
        assert_eq!(miss_category(&direct, None), "vocabulary");
        assert_eq!(miss_category(&direct, Some(9)), "ranking");
        let mut noisy = direct.clone();
        noisy[2] = p(3, "doc_section", "matched \"x\"");
        assert_eq!(miss_category(&noisy, Some(9)), "noise");
        let mut hopped = direct.clone();
        hopped[7] = p(8, "note", "similar to \"A\"");
        assert_eq!(miss_category(&hopped, Some(9)), "noise");
        // Only the seed slots above the target count.
        let mut late = direct;
        late.push(p(10, "doc_section", "matched \"x\""));
        late.push(p(11, "gotcha", "matched \"x\""));
        assert_eq!(miss_category(&late, Some(11)), "ranking");
    }

    #[test]
    fn deep_rank_finds_a_target_below_the_seed_cut() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let ids: Vec<i64> = (0..12).map(|n| note(&store, NodeKind::Gotcha, &format!("webhook signature {n}"), &format!("webhook signature {n}"))).collect();
        let backend = Backend::Local { store: Box::new(store), repo: "r".into(), embedder: None };
        let deep = backend.deep("webhook signature").unwrap();
        assert!(deep.len() > SEED_SLOTS, "{deep:?}");
        let last = deep.last().unwrap().node_id;
        assert_eq!(deep_rank(&backend, &deep, &[last]).unwrap(), Some(deep.len()));
        assert_eq!(deep_rank(&backend, &deep, &[999_999]).unwrap(), None);
        assert!(ids.contains(&last));
    }

    #[test]
    fn a_pinned_needs_review_or_missing_target_is_refused_by_case() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let pinned = note(&store, NodeKind::Gotcha, "pinned rule", "x");
        let unsettled = note(&store, NodeKind::Note, "unsettled", "x");
        store.set_curation("r", pinned, NodeProminence::Pinned, None).unwrap();
        store.set_curation("r", unsettled, NodeProminence::NeedsReview, Some("unclassified")).unwrap();
        let backend = Backend::Local { store: Box::new(store), repo: "r".into(), embedder: None };
        for (id, target, says) in [("c-pin", pinned, "pinned"), ("c-review", unsettled, "needs_review"), ("c-gone", 999_999, "doesn't exist")] {
            let err = evaluate(&backend, &[case(id, "x", vec![target])], None, 1).unwrap_err().to_string();
            assert!(err.contains(id) && err.contains(says), "{err}");
        }
    }

    #[test]
    fn run_writes_every_field_the_spec_names() {
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("widgets");
        std::fs::create_dir_all(repo_dir.join(".context")).unwrap();
        let (hit, other) = {
            let store = agentops_mcp::open_store(&repo_dir).unwrap();
            let repo = agentops_mcp::repo_name(&repo_dir);
            let add = |name: &str, content: &str| upsert_node(store.as_ref(), NewNode { kind: NodeKind::Gotcha, repo: repo.clone(), path: Some(format!("vault:{name}")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap();
            (add("Webhook signature fails", "webhook signature 401"), add("Unrelated", "nothing shared"))
        };
        let cases = format!("{}\n{}\n", json!({ "id": "c1", "prompt": "webhook signature", "mode": "plan", "targets": [hit] }), json!({ "id": "c2", "prompt": "webhook signature", "mode": "default", "targets": [other] }));
        std::fs::write(repo_dir.join(".context").join("recall-eval.jsonl"), cases).unwrap();

        run(&repo_dir, false, false, None, None, 2).unwrap();

        let out_dir = repo_dir.join(".context").join("recall-eval");
        let file = std::fs::read_dir(&out_dir).unwrap().next().unwrap().unwrap().path();
        let report: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        for c in report["cases"].as_array().unwrap() {
            for field in ["case_id", "mode", "first_hit_rank", "hit_type", "digest_in_seed", "seed_kinds", "latency_ms", "deep_rank", "miss_category"] {
                assert!(c.get(field).is_some(), "missing {field}: {c}");
            }
        }
        for field in ["hit_rate", "mrr", "p50_ms", "p95_ms"] {
            assert!(report["summary"].get(field).is_some(), "missing {field}: {report}");
        }
        assert_eq!(report["cases"][0]["hit_type"], "exact");
        assert_eq!(report["cases"][1]["mode"], "default");
        assert_eq!(report["cases"][1]["hit_type"], "miss");
    }
}
