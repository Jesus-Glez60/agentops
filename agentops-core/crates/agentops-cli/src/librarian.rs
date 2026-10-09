//! `agentops librarian` — evaluates whether a cheap model can reliably
//! catalogue recorded gotchas as portable vs project-specific (see
//! `agentops_llm::librarian`). Two steps, both local-only:
//!
//! 1. `export-labels` pulls every gotcha from the repo's remote AgentOps
//!    server (read-only GETs) and writes `notes.json` (full bodies, the
//!    classifier's input) plus `labels.csv` (blank `scope`/`technology`
//!    columns for a human to fill in — the answer key).
//! 2. `dry-run` classifies every labeled note with each model in a
//!    `models.toml`, then writes `report.json`/`report.md`: accuracy against
//!    the labels, JSON validity, tokens/cost/latency per classification, and
//!    a pass/fail decision section against optional thresholds.
//!
//! Nothing is written back to the server. Spend is measured through the
//! same `UsageRecorder` the server ledger uses, just never persisted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agentops_llm::librarian::{classify_for_library, LibrarianVerdict, Scope};
use agentops_llm::{JsonMode, LlmCallRecord, LlmConfig, LlmErrorKind, Provider, UsageRecorder};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// How many notes each model must classify cleanly before it's allowed to
/// run the full set — the smoke gate that verifies an unproven provider
/// (e.g. OpenRouter, whose docs couldn't be checked) actually returns
/// parseable verdicts and real usage numbers.
const SMOKE_GATE: usize = 5;
/// How many rewrites per model go into the manual good/needs-edit/wrong
/// check — the same notes for every model so they're comparable.
const REWRITE_SAMPLE: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteRecord {
    pub node_id: i64,
    pub title: String,
    pub body: String,
}

// ---------------------------------------------------------------- CSV ----

/// RFC 4180 quoting: wrap in quotes when the field has a comma, quote, or
/// newline; double any embedded quote.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Minimal RFC 4180 reader — quoted fields may contain commas, doubled
/// quotes, and newlines. Enough for a file this tool wrote and a person
/// edited in a spreadsheet; not a general-purpose CSV parser.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => in_quotes = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' => in_quotes = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

const LABELS_HEADER: &str = "node_id,title,body_excerpt,scope,technology";

fn excerpt(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 300 {
        flat
    } else {
        format!("{}…", flat.chars().take(300).collect::<String>())
    }
}

fn render_labels_csv(notes: &[NoteRecord]) -> String {
    let mut out = format!("{LABELS_HEADER}\n");
    for n in notes {
        out.push_str(&format!("{},{},{},,\n", n.node_id, csv_field(&n.title), csv_field(&excerpt(&n.body))));
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub scope: Scope,
    pub technology: Option<String>,
}

/// Rows with an empty `scope` are unlabeled and skipped; anything other
/// than `portable`/`project_specific` (case-insensitive, `-` or `_`) is a
/// hard error naming the row, rather than a silently dropped label.
fn parse_labels(text: &str) -> Result<BTreeMap<i64, Label>> {
    let mut labels = BTreeMap::new();
    for (i, row) in parse_csv(text).into_iter().enumerate().skip(1) {
        if row.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        let line = i + 1;
        let node_id: i64 = row.first().map(|s| s.trim()).unwrap_or("").parse().with_context(|| format!("labels.csv row {line}: node_id isn't a number"))?;
        let scope_raw = row.get(3).map(|s| s.trim().to_lowercase().replace('-', "_")).unwrap_or_default();
        let scope = match scope_raw.as_str() {
            "" => continue,
            "portable" => Scope::Portable,
            "project_specific" => Scope::ProjectSpecific,
            other => anyhow::bail!("labels.csv row {line} (node {node_id}): scope must be portable or project_specific, got {other:?}"),
        };
        let technology = row.get(4).map(|s| s.trim()).filter(|s| !s.is_empty()).map(String::from);
        labels.insert(node_id, Label { scope, technology });
    }
    Ok(labels)
}

// -------------------------------------------------------------- export ----

fn remote_get(server_url: &str, api_key: &str, path: &str) -> Result<serde_json::Value> {
    let url = format!("{server_url}{path}");
    let mut response = ureq::get(&url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .with_context(|| format!("calling GET {url}"))?;
    if !response.status().is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("GET {url} returned {}: {body}", response.status());
    }
    response.body_mut().read_json().with_context(|| format!("parsing GET {url} response"))
}

pub fn export_labels(server_url: &str, connection_id: &str, api_key: &str, out_dir: &Path) -> Result<usize> {
    let listing = remote_get(server_url, api_key, &format!("/gotchas?repos={connection_id}"))?;
    let gotchas = listing.get("gotchas").and_then(|g| g.as_array()).context("GET /gotchas response had no 'gotchas' array")?;

    let mut notes = Vec::with_capacity(gotchas.len());
    for g in gotchas {
        let node_id = g.get("id").and_then(|v| v.as_i64()).context("a gotcha in GET /gotchas had no numeric 'id'")?;
        let detail = remote_get(server_url, api_key, &format!("/repos/{connection_id}/nodes/{node_id}"))?;
        let title = detail.get("name").and_then(|v| v.as_str()).unwrap_or("(untitled)").to_string();
        let body = detail.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
        notes.push(NoteRecord { node_id, title, body });
        eprint!("\rfetched {}/{}", notes.len(), gotchas.len());
    }
    eprintln!();
    notes.sort_by_key(|n| n.node_id);

    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join("notes.json"), serde_json::to_string_pretty(&notes)?)?;
    let labels_path = out_dir.join("labels.csv");
    if labels_path.exists() {
        anyhow::bail!("{} already exists -- refusing to overwrite labels you may have filled in (notes.json was refreshed)", labels_path.display());
    }
    std::fs::write(&labels_path, render_labels_csv(&notes))?;
    Ok(notes.len())
}

// ------------------------------------------------------------- dry run ----

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JsonModeArg {
    JsonSchemaStrict,
    JsonSchema,
    JsonObject,
    GuidedJson,
}

impl From<JsonModeArg> for JsonMode {
    fn from(m: JsonModeArg) -> Self {
        match m {
            JsonModeArg::JsonSchemaStrict => JsonMode::JsonSchemaStrict,
            JsonModeArg::JsonSchema => JsonMode::JsonSchema,
            JsonModeArg::JsonObject => JsonMode::JsonObject,
            JsonModeArg::GuidedJson => JsonMode::GuidedJson,
        }
    }
}

/// One `[[model]]` entry. `provider = "anthropic"` uses the Messages API;
/// anything else is an OpenAI-compatible endpoint and needs `base_url` +
/// `json_mode`. Keys are only ever read from the environment variable
/// `api_key_env` names — never stored in this file.
#[derive(Debug, Clone, Deserialize)]
struct ModelEntry {
    name: String,
    provider: String,
    model: String,
    api_key_env: String,
    base_url: Option<String>,
    json_mode: Option<JsonModeArg>,
    #[serde(default)]
    min_interval_ms: u64,
}

#[derive(Debug, Deserialize)]
struct ModelsFile {
    model: Vec<ModelEntry>,
}

fn build_config(entry: &ModelEntry) -> Result<LlmConfig> {
    let api_key = std::env::var(&entry.api_key_env).with_context(|| format!("model {:?}: environment variable {} is not set", entry.name, entry.api_key_env))?;
    if entry.provider == "anthropic" {
        return Ok(LlmConfig { api_key, model: entry.model.clone(), ..Default::default() });
    }
    let base_url = entry.base_url.as_deref().with_context(|| format!("model {:?}: non-anthropic provider {:?} needs base_url", entry.name, entry.provider))?;
    let json_mode = entry.json_mode.clone().with_context(|| format!("model {:?}: non-anthropic provider {:?} needs json_mode", entry.name, entry.provider))?;
    Ok(LlmConfig {
        api_key,
        model: entry.model.clone(),
        api_url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
        provider: Provider::OpenAiCompatible { name: entry.provider.clone(), json_mode: json_mode.into() },
        ..Default::default()
    })
}

#[derive(Debug, Clone, Serialize)]
struct NoteOutcome {
    node_id: i64,
    verdict: Option<LibrarianVerdict>,
    error: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    latency_ms: u64,
    cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Confusion {
    pub portable_as_portable: usize,
    pub portable_as_project: usize,
    pub project_as_portable: usize,
    pub project_as_project: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelReport {
    name: String,
    provider: String,
    model: String,
    gated_out: Option<String>,
    attempted: usize,
    valid: usize,
    /// Correct scope calls / attempted — a failed call counts as wrong.
    scope_accuracy: f64,
    confusion: Confusion,
    /// Matches / notes whose label has a technology, among valid verdicts.
    technology_match_rate: Option<f64>,
    /// `None` when the model never got an answer back at all.
    json_validity_rate: Option<f64>,
    mean_input_tokens: f64,
    p95_input_tokens: u64,
    mean_output_tokens: f64,
    p95_output_tokens: u64,
    mean_cost_per_classification_usd: Option<f64>,
    projected_cost_all_notes_usd: Option<f64>,
    mean_latency_ms: f64,
    rate_limited_calls: usize,
    rate_limited_rate: f64,
    outcomes: Vec<NoteOutcome>,
}

fn normalize_technology(t: &str) -> String {
    t.trim().to_lowercase().replace([' ', '_'], "-")
}

fn p95(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
    sorted[idx.saturating_sub(1).min(sorted.len() - 1)]
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// Pure scoring over already-collected outcomes + raw call records, so the
/// arithmetic is unit-testable without any provider.
fn score(entry_name: &str, provider: &str, model: &str, gated_out: Option<String>, outcomes: Vec<NoteOutcome>, records: &[LlmCallRecord], labels: &BTreeMap<i64, Label>, total_notes: usize) -> ModelReport {
    let mut confusion = Confusion::default();
    let (mut correct, mut tech_hits, mut tech_total) = (0usize, 0usize, 0usize);
    for o in &outcomes {
        let label = &labels[&o.node_id];
        let Some(v) = &o.verdict else {
            confusion.failed += 1;
            continue;
        };
        match (label.scope, v.scope) {
            (Scope::Portable, Scope::Portable) => confusion.portable_as_portable += 1,
            (Scope::Portable, Scope::ProjectSpecific) => confusion.portable_as_project += 1,
            (Scope::ProjectSpecific, Scope::Portable) => confusion.project_as_portable += 1,
            (Scope::ProjectSpecific, Scope::ProjectSpecific) => confusion.project_as_project += 1,
        }
        if label.scope == v.scope {
            correct += 1;
        }
        if let Some(expected) = &label.technology {
            tech_total += 1;
            if v.technology.as_deref().map(normalize_technology) == Some(normalize_technology(expected)) {
                tech_hits += 1;
            }
        }
    }

    let attempted = outcomes.len();
    let valid = outcomes.iter().filter(|o| o.verdict.is_some()).count();
    let inputs: Vec<u64> = outcomes.iter().map(|o| o.input_tokens).collect();
    let outputs: Vec<u64> = outcomes.iter().map(|o| o.output_tokens).collect();
    let rate_limited_calls = records.iter().filter(|r| r.error_kind == Some(LlmErrorKind::RateLimited)).count();
    let bad_json_calls = records.iter().filter(|r| r.error_kind == Some(LlmErrorKind::BadJson)).count();
    let answered_calls = records.iter().filter(|r| r.success || r.error_kind == Some(LlmErrorKind::BadJson)).count();
    // A cost is only "known" if every classification had one -- one
    // unpriced note would otherwise silently lower the mean.
    let mean_cost = if !outcomes.is_empty() && outcomes.iter().all(|o| o.cost_usd.is_some()) { Some(mean(outcomes.iter().filter_map(|o| o.cost_usd))) } else { None };
    let ratio = |num: usize, den: usize| if den == 0 { 0.0 } else { num as f64 / den as f64 };

    ModelReport {
        name: entry_name.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        gated_out,
        attempted,
        valid,
        scope_accuracy: ratio(correct, attempted),
        confusion,
        technology_match_rate: (tech_total > 0).then(|| ratio(tech_hits, tech_total)),
        json_validity_rate: (answered_calls > 0).then(|| ratio(answered_calls - bad_json_calls, answered_calls)),
        mean_input_tokens: mean(inputs.iter().map(|&v| v as f64)),
        p95_input_tokens: p95(&inputs),
        mean_output_tokens: mean(outputs.iter().map(|&v| v as f64)),
        p95_output_tokens: p95(&outputs),
        mean_cost_per_classification_usd: mean_cost,
        projected_cost_all_notes_usd: mean_cost.map(|c| c * total_notes as f64),
        mean_latency_ms: mean(outcomes.iter().map(|o| o.latency_ms as f64)),
        rate_limited_calls,
        rate_limited_rate: ratio(rate_limited_calls, records.len()),
        outcomes,
    }
}

fn run_model(entry: &ModelEntry, notes: &[&NoteRecord], labels: &BTreeMap<i64, Label>, total_notes: usize) -> ModelReport {
    let config = match build_config(entry) {
        Ok(c) => c,
        Err(e) => return score(&entry.name, &entry.provider, &entry.model, Some(e.to_string()), Vec::new(), &[], labels, total_notes),
    };
    let recorder = UsageRecorder::default();
    let config = config.with_usage_sink(recorder.clone());

    let mut outcomes = Vec::new();
    let mut all_records = Vec::new();
    let mut gated_out = None;
    for (i, note) in notes.iter().enumerate() {
        if i > 0 && entry.min_interval_ms > 0 {
            std::thread::sleep(Duration::from_millis(entry.min_interval_ms));
        }
        eprint!("\r[{}] {}/{}", entry.name, i + 1, notes.len());
        let title = agentops_security::redact(&note.title).text;
        let body = agentops_security::redact(&note.body).text;
        let result = classify_for_library(&config, &title, &body);
        let records = recorder.take();
        let cost: Option<f64> = records.iter().map(|r| agentops_llm::pricing::cost_estimate_usd(&r.provider, &r.model, r.input_tokens as i64, r.output_tokens as i64, 0, 0)).sum();
        let outcome = NoteOutcome {
            node_id: note.node_id,
            input_tokens: records.iter().map(|r| r.input_tokens).sum(),
            output_tokens: records.iter().map(|r| r.output_tokens).sum(),
            latency_ms: records.iter().map(|r| r.latency_ms).sum(),
            cost_usd: cost,
            verdict: result.as_ref().ok().cloned(),
            error: result.as_ref().err().map(|e| format!("{e:#}")),
        };
        let smoke_failure = i < SMOKE_GATE && (outcome.verdict.is_none() || outcome.input_tokens == 0);
        all_records.extend(records);
        outcomes.push(outcome);
        if smoke_failure {
            let o = outcomes.last().unwrap();
            gated_out = Some(match &o.error {
                Some(e) => format!("smoke gate: note {} failed: {e}", o.node_id),
                None => format!("smoke gate: note {} returned zero usage tokens -- the provider's usage numbers can't be trusted", o.node_id),
            });
            break;
        }
    }
    eprintln!();
    score(&entry.name, &entry.provider, &entry.model, gated_out, outcomes, &all_records, labels, total_notes)
}

#[derive(Debug, Clone, Default)]
pub struct Thresholds {
    pub min_accuracy: Option<f64>,
    pub max_cost_usd: Option<f64>,
    pub max_rate_limited_rate: Option<f64>,
}

fn verdict_line(report: &ModelReport, t: &Thresholds) -> String {
    if let Some(reason) = &report.gated_out {
        return format!("**{}**: gated out — {reason}", report.name);
    }
    let check = |name: &str, threshold: Option<String>, pass: Option<bool>| match (threshold, pass) {
        (None, _) => format!("{name}: not set"),
        (Some(th), Some(true)) => format!("{name} {th}: PASS"),
        (Some(th), Some(false)) => format!("{name} {th}: FAIL"),
        (Some(th), None) => format!("{name} {th}: UNKNOWN (no rate for this model)"),
    };
    let parts = [
        check("accuracy ≥", t.min_accuracy.map(|v| format!("{:.0}%", v * 100.0)), t.min_accuracy.map(|v| report.scope_accuracy >= v)),
        check("cost/classification ≤", t.max_cost_usd.map(|v| format!("${v}")), t.max_cost_usd.and_then(|v| report.mean_cost_per_classification_usd.map(|c| c <= v))),
        check("429 rate ≤", t.max_rate_limited_rate.map(|v| format!("{:.0}%", v * 100.0)), t.max_rate_limited_rate.map(|v| report.rate_limited_rate <= v)),
    ];
    format!("**{}**: {}", report.name, parts.join(" · "))
}

fn fmt_cost(c: Option<f64>) -> String {
    c.map(|v| format!("${v:.5}")).unwrap_or_else(|| "unknown".to_string())
}

fn render_markdown(reports: &[ModelReport], notes: &[NoteRecord], labels: &BTreeMap<i64, Label>, rewrite_ids: &[i64], thresholds: &Thresholds) -> String {
    let mut md = String::from("# Librarian dry run\n\n");
    md.push_str("Raw token counts aren't comparable across providers (tokenizers differ; Claude 4.7+ alone produces ~30% more tokens for the same text) — compare accuracy and cost.\n\n");

    md.push_str("## Decision\n\n");
    for r in reports {
        md.push_str(&format!("- {}\n", verdict_line(r, thresholds)));
    }

    md.push_str("\n## Per model\n\n| Model | Attempted | Scope accuracy | Tech match | JSON valid | Mean in/out tokens | p95 in/out | Cost / classification | Projected (all notes) | Mean latency | 429s |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
    for r in reports {
        md.push_str(&format!(
            "| {} ({}/{}){} | {} | {:.1}% | {} | {} | {:.0} / {:.0} | {} / {} | {} | {} | {:.0} ms | {} |\n",
            r.name,
            r.provider,
            r.model,
            if r.gated_out.is_some() { " ⛔" } else { "" },
            r.attempted,
            r.scope_accuracy * 100.0,
            r.technology_match_rate.map(|v| format!("{:.1}%", v * 100.0)).unwrap_or_else(|| "—".into()),
            r.json_validity_rate.map(|v| format!("{:.1}%", v * 100.0)).unwrap_or_else(|| "—".into()),
            r.mean_input_tokens,
            r.mean_output_tokens,
            r.p95_input_tokens,
            r.p95_output_tokens,
            fmt_cost(r.mean_cost_per_classification_usd),
            fmt_cost(r.projected_cost_all_notes_usd),
            r.mean_latency_ms,
            r.rate_limited_calls,
        ));
    }

    md.push_str("\n## Confusion (label → model)\n\n| Model | portable→portable | portable→project | project→portable | project→project | failed |\n|---|---|---|---|---|---|\n");
    for r in reports {
        let c = &r.confusion;
        md.push_str(&format!("| {} | {} | {} | {} | {} | {} |\n", r.name, c.portable_as_portable, c.portable_as_project, c.project_as_portable, c.project_as_project, c.failed));
    }

    md.push_str("\n## Disagreements with your labels\n\n");
    let by_id: BTreeMap<i64, &NoteRecord> = notes.iter().map(|n| (n.node_id, n)).collect();
    for r in reports {
        let wrong: Vec<String> = r
            .outcomes
            .iter()
            .filter_map(|o| match &o.verdict {
                None => Some((o.node_id, "FAILED".to_string())),
                Some(v) if v.scope != labels[&o.node_id].scope => Some((o.node_id, format!("{:?} (you said {:?})", v.scope, labels[&o.node_id].scope))),
                Some(_) => None,
            })
            .map(|(id, got)| format!("- {id} \"{}\" → {got}", by_id.get(&id).map(|n| n.title.as_str()).unwrap_or("?")))
            .collect();
        md.push_str(&format!("### {}\n\n{}\n\n", r.name, if wrong.is_empty() { "None.".to_string() } else { wrong.join("\n") }));
    }

    md.push_str("## Rewrite check\n\nThe same notes for every model. Mark each as good / needs-edit / wrong.\n\n");
    for id in rewrite_ids {
        let Some(note) = by_id.get(id) else { continue };
        md.push_str(&format!("### Note {id}: {}\n\n<details><summary>Original</summary>\n\n{}\n\n</details>\n\n", note.title, note.body));
        for r in reports {
            match r.outcomes.iter().find(|o| o.node_id == *id).and_then(|o| o.verdict.as_ref()) {
                Some(v) => md.push_str(&format!("**{}** — mark: ____\n\n> **{}**\n>\n> {}\n\n", r.name, v.generalized_title, v.generalized_body.replace('\n', "\n> "))),
                None => md.push_str(&format!("**{}** — no verdict\n\n", r.name)),
            }
        }
    }
    md
}

pub fn dry_run(models_path: &Path, labels_dir: &Path, out_dir: &Path, limit: Option<usize>, thresholds: &Thresholds) -> Result<PathBuf> {
    let models: ModelsFile = toml::from_str(&std::fs::read_to_string(models_path).with_context(|| format!("reading {}", models_path.display()))?).with_context(|| format!("parsing {}", models_path.display()))?;
    let notes: Vec<NoteRecord> = serde_json::from_str(&std::fs::read_to_string(labels_dir.join("notes.json")).context("reading notes.json -- run `agentops librarian export-labels` first")?)?;
    let labels = parse_labels(&std::fs::read_to_string(labels_dir.join("labels.csv")).context("reading labels.csv")?)?;
    if labels.is_empty() {
        anyhow::bail!("labels.csv has no labeled rows yet -- fill in the scope column (portable / project_specific) first");
    }

    let mut labeled: Vec<&NoteRecord> = notes.iter().filter(|n| labels.contains_key(&n.node_id)).collect();
    if let Some(n) = limit {
        labeled.truncate(n);
    }
    let rewrite_ids: Vec<i64> = labeled.iter().take(REWRITE_SAMPLE).map(|n| n.node_id).collect();
    eprintln!("{} labeled note(s) of {} total; {} model(s)", labeled.len(), notes.len(), models.model.len());

    let reports: Vec<ModelReport> = models.model.iter().map(|m| run_model(m, &labeled, &labels, notes.len())).collect();

    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join("report.json"), serde_json::to_string_pretty(&reports)?)?;
    let md_path = out_dir.join("report.md");
    std::fs::write(&md_path, render_markdown(&reports, &notes, &labels, &rewrite_ids, thresholds))?;
    Ok(md_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_round_trips_commas_quotes_and_newlines() {
        let notes = vec![NoteRecord { node_id: 7, title: "a, \"b\"".into(), body: "line one\nline two".into() }];
        let rows = parse_csv(&render_labels_csv(&notes));
        assert_eq!(rows[0].join(","), LABELS_HEADER);
        assert_eq!(rows[1], vec!["7", "a, \"b\"", "line one line two", "", ""]);
    }

    #[test]
    fn parse_labels_skips_blank_scopes_and_rejects_unknown_ones() {
        let text = format!("{LABELS_HEADER}\n1,t,b,portable,SQLite\n2,t,b,,\n3,t,b,Project-Specific,\n");
        let labels = parse_labels(&text).unwrap();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[&1], Label { scope: Scope::Portable, technology: Some("SQLite".into()) });
        assert_eq!(labels[&3].scope, Scope::ProjectSpecific);

        let bad = format!("{LABELS_HEADER}\n4,t,b,maybe,\n");
        assert!(parse_labels(&bad).unwrap_err().to_string().contains("node 4"));
    }

    fn outcome(node_id: i64, scope: Option<Scope>, tech: Option<&str>, cost: Option<f64>) -> NoteOutcome {
        NoteOutcome {
            node_id,
            verdict: scope.map(|scope| LibrarianVerdict { scope, technology: tech.map(String::from), version: None, generalized_title: "t".into(), generalized_body: "b".into(), rationale: "r".into() }),
            error: scope.is_none().then(|| "failed".to_string()),
            input_tokens: 100,
            output_tokens: 20,
            latency_ms: 300,
            cost_usd: cost,
        }
    }

    #[test]
    fn scoring_counts_failures_as_wrong_and_normalizes_technology() {
        let labels: BTreeMap<i64, Label> = [
            (1, Label { scope: Scope::Portable, technology: Some("css grid".into()) }),
            (2, Label { scope: Scope::ProjectSpecific, technology: None }),
            (3, Label { scope: Scope::Portable, technology: Some("sqlite".into()) }),
            (4, Label { scope: Scope::Portable, technology: None }),
        ]
        .into();
        let outcomes = vec![
            outcome(1, Some(Scope::Portable), Some("CSS-Grid"), Some(0.001)),
            outcome(2, Some(Scope::Portable), None, Some(0.001)),
            outcome(3, Some(Scope::Portable), Some("rusqlite"), Some(0.001)),
            outcome(4, None, None, Some(0.0)),
        ];
        let rec = |kind: Option<LlmErrorKind>| LlmCallRecord { operation: "x".into(), provider: "anthropic".into(), model: "m".into(), input_tokens: 1, output_tokens: 1, latency_ms: 1, success: kind.is_none(), error_kind: kind };
        let records = vec![rec(None), rec(None), rec(None), rec(Some(LlmErrorKind::RateLimited)), rec(Some(LlmErrorKind::BadJson))];

        let r = score("m", "anthropic", "m", None, outcomes, &records, &labels, 135);
        assert_eq!(r.attempted, 4);
        assert!((r.scope_accuracy - 0.5).abs() < 1e-9, "2 correct of 4 attempted (the failure counts as wrong)");
        assert_eq!(r.confusion, Confusion { portable_as_portable: 2, portable_as_project: 0, project_as_portable: 1, project_as_project: 0, failed: 1 });
        assert_eq!(r.technology_match_rate, Some(0.5), "css grid == CSS-Grid after normalizing; sqlite != rusqlite");
        assert_eq!(r.rate_limited_calls, 1);
        assert!((r.json_validity_rate.unwrap() - 0.75).abs() < 1e-9, "1 bad-JSON of 4 answered calls");
        assert!((r.projected_cost_all_notes_usd.unwrap() - 0.00075 * 135.0).abs() < 1e-9);
    }

    #[test]
    fn one_unpriced_classification_makes_the_mean_cost_unknown() {
        let labels: BTreeMap<i64, Label> = [(1, Label { scope: Scope::Portable, technology: None }), (2, Label { scope: Scope::Portable, technology: None })].into();
        let r = score("m", "groq", "m", None, vec![outcome(1, Some(Scope::Portable), None, Some(0.01)), outcome(2, Some(Scope::Portable), None, None)], &[], &labels, 2);
        assert_eq!(r.mean_cost_per_classification_usd, None);
    }

    #[test]
    fn p95_picks_the_nearest_rank() {
        assert_eq!(p95(&(1..=20).collect::<Vec<u64>>()), 19);
        assert_eq!(p95(&[5]), 5);
        assert_eq!(p95(&[]), 0);
    }
}
