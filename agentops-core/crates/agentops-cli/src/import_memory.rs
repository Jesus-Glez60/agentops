//! `agentops import-memory`: imports a Claude Code auto-memory directory
//! (MEMORY.md plus one file per entry) into AgentOps (milestone 2 of the
//! cue-driven memory work). Without `--apply` it changes nothing and only
//! writes the report.
//!
//! Every entry is classified into a memory kind, first match wins:
//! frontmatter `type` (top level or under `metadata`), then the file-name
//! prefix, then the heuristic classifier or `--llm-classify` at
//! `CLASSIFY_MIN_CONFIDENCE`, else `NeedsReview` ("unclassified on
//! import"). An entry that says it's stale or superseded also goes to
//! `NeedsReview`. Every entry gets one report record: entries in equals
//! records out.
//!
//! Writes go through `add_note` (over `/mcp` when the repo has a remote
//! marker, else locally), so the importer and an agent writing a note
//! behave the same: embeddings, similarity edges, duplicates. The memory
//! directory itself is only ever read; `--apply` first copies it to
//! `.context/memory-backup/<date>/`. Reports stay under `.context/`
//! (git-ignored), never `docs/`: entries are personal and client data.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use agentops_mcp::AddMode;
use agentops_notes::NoteType;
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::remote_marker::{read_remote_marker, RemoteMcpMarker};

/// Below this classifier confidence, a person decides (`NeedsReview`).
pub(crate) const CLASSIFY_MIN_CONFIDENCE: f32 = 0.7;
/// Tag every imported node carries, beside `memory-kind:<kind>`.
pub(crate) const IMPORTED_TAG: &str = "imported:memory-md";
const UNCLASSIFIED_REASON: &str = "unclassified on import";
const STALE_REASON: &str = "says it is stale or superseded";
/// Words that mark an entry as no longer current (matched case-insensitively).
const STALE_WORDS: &[&str] = &["stale", "superseded", "no longer", "obsolete", "outdated"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MemoryKind {
    Semantic,
    Episodic,
    Procedural,
}

impl MemoryKind {
    fn from_claude_type(t: &str) -> Option<Self> {
        match t.trim().to_lowercase().as_str() {
            "feedback" => Some(MemoryKind::Procedural),
            "project" => Some(MemoryKind::Episodic),
            "reference" | "user" => Some(MemoryKind::Semantic),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            MemoryKind::Semantic => "semantic",
            MemoryKind::Episodic => "episodic",
            MemoryKind::Procedural => "procedural",
        }
    }
}

/// One entry file, as read. Only the fields the importer uses.
#[derive(Debug, Default)]
struct Entry {
    file: String,
    name: Option<String>,
    description: Option<String>,
    claude_type: Option<String>,
    modified: Option<String>,
    body: String,
    warnings: Vec<String>,
}

/// Reads frontmatter by hand: top-level `name`, `description`, `type`, and
/// `type`/`modified` nested under `metadata:`. An opening `---` with no
/// closing one is malformed: the whole file becomes the body, with a
/// warning, and classification falls through to the prefix.
fn parse_entry(file: &str, content: &str) -> Entry {
    let mut entry = Entry { file: file.to_string(), ..Default::default() };
    let Some(rest) = content.strip_prefix("---\n") else {
        entry.body = content.trim().to_string();
        return entry;
    };
    let Some(end) = rest.find("\n---") else {
        entry.warnings.push("malformed frontmatter (no closing ---)".into());
        entry.body = content.trim().to_string();
        return entry;
    };
    let (front, after) = rest.split_at(end);
    entry.body = after.trim_start_matches("\n---").trim().to_string();
    let mut in_metadata = false;
    for line in front.lines() {
        let indented = line.starts_with(' ') || line.starts_with('\t');
        let Some((key, value)) = line.trim().split_once(':') else { continue };
        let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
        if !indented {
            in_metadata = key == "metadata";
        }
        match (indented && in_metadata, key) {
            (false, "name") => entry.name = Some(value).filter(|v| !v.is_empty()),
            (false, "description") => entry.description = Some(value).filter(|v| !v.is_empty()),
            (_, "type") => entry.claude_type = entry.claude_type.take().or(Some(value).filter(|v| !v.is_empty())),
            (true, "modified") => entry.modified = Some(value),
            _ => {}
        }
    }
    entry
}

/// `MEMORY.md`'s `- [Title](file.md)` lines: file name → line number.
fn index_lines(memory_md: &str) -> HashMap<String, usize> {
    memory_md
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let start = line.find("](")? + 2;
            let end = line[start..].find(')')? + start;
            Some((line[start..end].to_string(), i + 1))
        })
        .collect()
}

/// How an entry was classified, before any write.
#[derive(Debug, Clone, PartialEq)]
struct Classified {
    signal: &'static str,
    memory_kind: Option<MemoryKind>,
    /// A node kind the classifier already chose (heuristic or LLM), else
    /// `add_note` classifies a semantic entry itself.
    note_type: Option<NoteType>,
    confidence: Option<f32>,
    needs_review: Option<&'static str>,
}

/// An LLM verdict for an entry with no type and no prefix.
pub(crate) trait KindClassifier {
    fn classify(&self, title: &str, body: &str) -> Result<(Option<MemoryKind>, f32)>;
}

fn classify(entry: &Entry, llm: Option<&dyn KindClassifier>) -> Result<Classified> {
    let stale = {
        let text = format!("{} {}", entry.description.as_deref().unwrap_or(""), entry.body).to_lowercase();
        STALE_WORDS.iter().any(|w| text.contains(w))
    };
    let prefix = ["feedback", "project", "reference", "user"].into_iter().find(|p| entry.file.starts_with(&format!("{p}_")));
    let mut classified = if let Some(kind) = entry.claude_type.as_deref().and_then(MemoryKind::from_claude_type) {
        Classified { signal: "frontmatter", memory_kind: Some(kind), note_type: None, confidence: None, needs_review: None }
    } else if let Some(kind) = prefix.and_then(MemoryKind::from_claude_type) {
        Classified { signal: "prefix", memory_kind: Some(kind), note_type: None, confidence: None, needs_review: None }
    } else if let Some(llm) = llm {
        let (kind, confidence) = llm.classify(entry.name.as_deref().unwrap_or(&entry.file), &entry.body)?;
        match kind.filter(|_| confidence >= CLASSIFY_MIN_CONFIDENCE) {
            Some(kind) => Classified { signal: "llm", memory_kind: Some(kind), note_type: None, confidence: Some(confidence), needs_review: None },
            None => Classified { signal: "llm", memory_kind: None, note_type: None, confidence: Some(confidence), needs_review: Some(UNCLASSIFIED_REASON) },
        }
    } else {
        // The heuristic has no score, only "found gotcha/decision language
        // or not": a hit counts as exactly the minimum confidence.
        match agentops_notes::heuristic_score(&entry.body) {
            (note_type, true) => Classified { signal: "heuristic", memory_kind: Some(MemoryKind::Semantic), note_type: Some(note_type), confidence: Some(CLASSIFY_MIN_CONFIDENCE), needs_review: None },
            (_, false) => Classified { signal: "none", memory_kind: None, note_type: None, confidence: Some(0.0), needs_review: Some(UNCLASSIFIED_REASON) },
        }
    };
    if stale && classified.needs_review.is_none() {
        classified.needs_review = Some(STALE_REASON);
    }
    Ok(classified)
}

/// What one `add_note` call reported.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NoteOutcome {
    pub(crate) note_type: String,
    pub(crate) node_id: Option<i64>,
    pub(crate) written: bool,
    pub(crate) already_exists: bool,
    pub(crate) duplicate_of: Option<i64>,
    pub(crate) similar_to: Vec<i64>,
}

/// Where imported notes go: this repo's store, or its server connection.
pub(crate) trait Sink {
    fn add_note(&self, title: &str, body: &str, note_type: Option<NoteType>, tags: &[String], mode: AddMode) -> Result<NoteOutcome>;
    /// `prominence` is a REST value: `needs_review`, `pinned`, ...
    fn set_curation(&self, node_id: i64, prominence: &str, reason: Option<&str>) -> Result<()>;
}

pub(crate) struct LocalSink {
    pub(crate) repo: PathBuf,
}

impl Sink for LocalSink {
    fn add_note(&self, title: &str, body: &str, note_type: Option<NoteType>, tags: &[String], mode: AddMode) -> Result<NoteOutcome> {
        let r = agentops_mcp::add_note_with(&self.repo, title, body, note_type, tags, None, true, mode)?;
        Ok(NoteOutcome { note_type: note_type_str(r.note_type).into(), node_id: r.node_id, written: r.written, already_exists: r.already_exists, duplicate_of: r.duplicate_of.map(|(id, _)| id), similar_to: r.similar_to.iter().map(|(id, _)| *id).collect() })
    }

    fn set_curation(&self, node_id: i64, prominence: &str, reason: Option<&str>) -> Result<()> {
        let store = agentops_mcp::open_store(&self.repo)?;
        let prominence = match prominence {
            "needs_review" => agentops_graph::NodeProminence::NeedsReview,
            "pinned" => agentops_graph::NodeProminence::Pinned,
            other => anyhow::bail!("unsupported prominence {other:?}"),
        };
        store.set_curation(&agentops_mcp::repo_name(&self.repo), node_id, prominence, reason)
    }
}

pub(crate) struct RemoteSink {
    pub(crate) marker: RemoteMcpMarker,
}

impl Sink for RemoteSink {
    fn add_note(&self, title: &str, body: &str, note_type: Option<NoteType>, tags: &[String], mode: AddMode) -> Result<NoteOutcome> {
        let mut args = json!({ "path": self.marker.connection_id, "title": title, "body": body, "tags": tags, "with_embeddings": true });
        if let Some(t) = note_type {
            args["note_type"] = json!(note_type_str(t));
        }
        match mode {
            AddMode::DryRun => args["dry_run"] = json!(true),
            AddMode::WriteUnlessDuplicate => args["skip_duplicates"] = json!(true),
            AddMode::Write => {}
        }
        let text = crate::mcp_client::call_tool(&self.marker.server_url, &self.marker.api_key, "add_note", args)?;
        parse_add_note_result(&text)
    }

    fn set_curation(&self, node_id: i64, prominence: &str, reason: Option<&str>) -> Result<()> {
        let url = format!("{}/repos/{}/nodes/{node_id}/curation", self.marker.server_url.trim_end_matches('/'), self.marker.connection_id);
        let mut response = ureq::post(&url)
            .header("Authorization", &format!("Bearer {}", self.marker.api_key))
            .config()
            .http_status_as_error(false)
            .build()
            .send_json(json!({ "prominence": prominence, "reason": reason }))
            .with_context(|| format!("calling POST {url}"))?;
        if !response.status().is_success() {
            let status = response.status();
            anyhow::bail!("POST {url} returned {status}: {}", response.body_mut().read_to_string().unwrap_or_default());
        }
        Ok(())
    }
}

/// The `result: {json}` line `add_note` adds when `dry_run` or
/// `skip_duplicates` is set.
fn parse_add_note_result(text: &str) -> Result<NoteOutcome> {
    let line = text.lines().rev().find_map(|l| l.strip_prefix("result: ")).ok_or_else(|| anyhow::anyhow!("add_note returned no result line (is the server older than milestone 2?): {text}"))?;
    let v: Value = serde_json::from_str(line).context("parsing add_note's result line")?;
    let ids = |key: &str| v[key].as_array().map(|a| a.iter().filter_map(|x| x["node_id"].as_i64()).collect()).unwrap_or_default();
    Ok(NoteOutcome {
        note_type: v["note_type"].as_str().unwrap_or("knowledge").to_string(),
        node_id: v["node_id"].as_i64(),
        written: v["written"].as_bool().unwrap_or(false),
        already_exists: v["already_exists"].as_bool().unwrap_or(false),
        duplicate_of: v["duplicate_of"]["node_id"].as_i64(),
        similar_to: ids("similar_to"),
    })
}

fn note_type_str(t: NoteType) -> &'static str {
    match t {
        NoteType::Gotcha => "gotcha",
        NoteType::Decision => "decision",
        NoteType::Knowledge | NoteType::Context => "knowledge",
    }
}

/// One report record per entry (the spec's schema).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct Record {
    pub(crate) source_file: String,
    /// The `MEMORY.md` line that links this file; `None` if unindexed.
    pub(crate) source_line: Option<usize>,
    pub(crate) title: String,
    pub(crate) signal: &'static str,
    pub(crate) memory_kind: Option<MemoryKind>,
    pub(crate) node_kind: Option<String>,
    pub(crate) confidence: Option<f32>,
    /// `create`, `duplicate`, `needs_review` or `skip`.
    pub(crate) action: &'static str,
    pub(crate) node_id: Option<i64>,
    pub(crate) duplicate_of: Option<i64>,
    pub(crate) pinned: bool,
    pub(crate) similar_to: Vec<i64>,
    pub(crate) warnings: Vec<String>,
}

/// The note text for an entry: its one-line description first (unless the
/// body already starts with it), then the body. An episodic entry keeps
/// its date.
fn note_body(entry: &Entry, kind: Option<MemoryKind>, mtime_date: &str) -> String {
    let mut body = match &entry.description {
        Some(d) if !entry.body.starts_with(d.as_str()) => format!("{d}\n\n{}", entry.body),
        _ => entry.body.clone(),
    };
    if kind == Some(MemoryKind::Episodic) {
        let date = entry.modified.as_deref().map(|m| m.chars().take(10).collect::<String>()).unwrap_or_else(|| mtime_date.to_string());
        body.push_str(&format!("\n\nRecorded {date} (imported from MEMORY.md)."));
    }
    body.trim().to_string()
}

/// Classifies and (with `apply`) writes every entry in `memory_dir`.
/// Returns one record per entry file, sorted by file name.
pub(crate) fn import(memory_dir: &Path, sink: &dyn Sink, llm: Option<&dyn KindClassifier>, apply: bool) -> Result<Vec<Record>> {
    let index = std::fs::read_to_string(memory_dir.join("MEMORY.md")).map(|s| index_lines(&s)).unwrap_or_default();
    let mut files: Vec<PathBuf> = std::fs::read_dir(memory_dir)
        .with_context(|| format!("reading {}", memory_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md") && p.file_name().is_some_and(|n| n != "MEMORY.md"))
        .collect();
    files.sort();

    let mut records = Vec::new();
    for path in files {
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        let source_line = index.get(&file).copied();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                records.push(skipped(&file, source_line, &file, vec![format!("unreadable: {e}")]));
                continue;
            }
        };
        let mut entry = parse_entry(&file, &content);
        if source_line.is_none() {
            entry.warnings.push("not linked from MEMORY.md".into());
        }
        let title = entry.name.clone().unwrap_or_else(|| file.trim_end_matches(".md").to_string());
        if entry.body.is_empty() && entry.description.is_none() {
            records.push(skipped(&file, source_line, &title, [entry.warnings, vec!["empty entry".into()]].concat()));
            continue;
        }
        let classified = classify(&entry, llm)?;
        let mtime_date = std::fs::metadata(&path).and_then(|m| m.modified()).map(utc_date).unwrap_or_default();
        let body = note_body(&entry, classified.memory_kind, &mtime_date);
        let note_type = match classified.memory_kind {
            Some(MemoryKind::Semantic) if classified.needs_review.is_none() => classified.note_type,
            _ => Some(NoteType::Knowledge),
        };
        let mut tags = vec![IMPORTED_TAG.to_string()];
        if let Some(kind) = classified.memory_kind {
            tags.push(format!("memory-kind:{}", kind.as_str()));
        }

        let preview = sink.add_note(&title, &body, note_type, &tags, AddMode::DryRun)?;
        let mut record = Record {
            source_file: file.clone(),
            source_line,
            title: title.clone(),
            signal: classified.signal,
            memory_kind: classified.memory_kind,
            node_kind: Some(preview.note_type.clone()),
            confidence: classified.confidence,
            action: "create",
            node_id: preview.node_id,
            duplicate_of: preview.duplicate_of,
            pinned: false,
            similar_to: preview.similar_to.clone(),
            warnings: entry.warnings,
        };
        if preview.already_exists {
            record.action = "skip";
            record.warnings.push(format!("already imported as node {}", preview.node_id.unwrap_or_default()));
        } else if preview.duplicate_of.is_some() {
            record.action = "duplicate";
        } else if classified.needs_review.is_some() {
            record.action = "needs_review";
        }
        if let Some(reason) = classified.needs_review {
            record.warnings.push(format!("needs review: {reason}"));
        }

        if apply && matches!(record.action, "create" | "needs_review") {
            let written = sink.add_note(&title, &body, note_type, &tags, AddMode::WriteUnlessDuplicate)?;
            record.node_id = written.node_id;
            record.similar_to = written.similar_to;
            if !written.written {
                record.action = "duplicate";
                record.duplicate_of = written.duplicate_of;
            } else if let (Some(reason), Some(id)) = (classified.needs_review, written.node_id) {
                sink.set_curation(id, "needs_review", Some(reason))?;
            }
        }
        records.push(record);
    }
    Ok(records)
}

fn skipped(file: &str, source_line: Option<usize>, title: &str, warnings: Vec<String>) -> Record {
    Record { source_file: file.into(), source_line, title: title.into(), signal: "none", memory_kind: None, node_kind: None, confidence: None, action: "skip", node_id: None, duplicate_of: None, pinned: false, similar_to: Vec::new(), warnings }
}

/// Counts and warnings only: no entry text, so it can be quoted safely.
pub(crate) fn summary_markdown(records: &[Record], apply: bool) -> String {
    let by = |f: &dyn Fn(&Record) -> String| -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for r in records {
            *m.entry(f(r)).or_insert(0) += 1;
        }
        m
    };
    let table = |title: &str, m: BTreeMap<String, usize>| format!("## {title}\n\n{}\n", m.iter().map(|(k, v)| format!("- {k}: {v}")).collect::<Vec<_>>().join("\n"));
    let mut out = format!("# MEMORY.md import ({})\n\n{} entries, {} records.\n\n", if apply { "applied" } else { "dry run" }, records.len(), records.len());
    out.push_str(&table("By action", by(&|r| r.action.to_string())));
    out.push_str(&table("By memory kind", by(&|r| r.memory_kind.map_or("unclassified".into(), |k| k.as_str().to_string()))));
    out.push_str(&table("By signal", by(&|r| r.signal.to_string())));
    out.push_str(&table("By node kind", by(&|r| r.node_kind.clone().unwrap_or_else(|| "none".into()))));
    let warnings: Vec<String> = records.iter().flat_map(|r| r.warnings.iter().map(move |w| format!("- `{}`: {w}", r.source_file))).collect();
    out.push_str(&format!("\n## Warnings ({})\n\n{}\n", warnings.len(), if warnings.is_empty() { "None.".to_string() } else { warnings.join("\n") }));
    out
}

/// `YYYY-MM-DD` (UTC) for a timestamp, Howard Hinnant's civil-from-days.
pub(crate) fn utc_date(t: std::time::SystemTime) -> String {
    let days = (t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Claude Code's per-project memory directory for `project`: its path with
/// every non-alphanumeric character as `-`, under `~/.claude/projects/`.
pub(crate) fn default_memory_dir(project: &Path) -> Result<PathBuf> {
    let project = std::fs::canonicalize(project).with_context(|| format!("resolving {}", project.display()))?;
    let encoded: String = project.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(home.join(".claude").join("projects").join(encoded).join("memory"))
}

/// Uses the Anthropic API (cheap tier) when `--llm-classify` is set.
struct LlmKindClassifier {
    config: agentops_llm::LlmConfig,
}

impl KindClassifier for LlmKindClassifier {
    fn classify(&self, title: &str, body: &str) -> Result<(Option<MemoryKind>, f32)> {
        let schema = json!({
            "type": "object",
            "properties": {
                "memory_kind": { "type": "string", "enum": ["semantic", "episodic", "procedural", "unknown"] },
                "confidence": { "type": "number" }
            },
            "required": ["memory_kind", "confidence"],
            "additionalProperties": false
        });
        let redacted = agentops_security::redact(body).text;
        let prompt = format!(
            "Classify this memory an AI coding assistant saved about a software project.\n\
             - semantic: a fact and the reason behind it (how something works, where something is).\n\
             - episodic: what happened, when, in what context (a session, a release, an incident).\n\
             - procedural: a rule that must always hold (always/never do X).\n\
             - unknown: none of these fits.\n\
             confidence: 0 to 1. Respond with JSON only.\n\nTitle: {title}\n\nBody:\n{redacted}"
        );
        let result = agentops_llm::call_json(&self.config, "import_memory_classify", &prompt, schema)?;
        let v: Value = serde_json::from_str(&result.text).context("parsing the classifier's JSON")?;
        let kind = match v["memory_kind"].as_str() {
            Some("semantic") => Some(MemoryKind::Semantic),
            Some("episodic") => Some(MemoryKind::Episodic),
            Some("procedural") => Some(MemoryKind::Procedural),
            _ => None,
        };
        Ok((kind, v["confidence"].as_f64().unwrap_or(0.0) as f32))
    }
}

/// The `import-memory` command.
pub(crate) fn run(project: &Path, memory_dir: Option<&Path>, apply: bool, report: Option<&Path>, llm_classify: bool) -> Result<()> {
    let memory_dir = match memory_dir {
        Some(d) => d.to_path_buf(),
        None => default_memory_dir(project)?,
    };
    let llm = if llm_classify { Some(LlmKindClassifier { config: agentops_llm::LlmConfig::from_env_cheap()? }) } else { None };
    let sink: Box<dyn Sink> = match read_remote_marker(project) {
        Some(marker) => {
            println!("importing into the server connection {} ({})", marker.connection_id, marker.server_url);
            Box::new(RemoteSink { marker })
        }
        None => {
            println!("importing into this repo's local graph");
            Box::new(LocalSink { repo: project.to_path_buf() })
        }
    };
    let date = utc_date(std::time::SystemTime::now());
    if apply {
        let backup = unique_dir(&project.join(".context").join("memory-backup").join(&date));
        std::fs::create_dir_all(&backup)?;
        for entry in std::fs::read_dir(&memory_dir)? {
            let path = entry?.path();
            if path.is_file() {
                std::fs::copy(&path, backup.join(path.file_name().unwrap()))?;
            }
        }
        println!("backed up {} to {}", memory_dir.display(), backup.display());
    }

    let records = import(&memory_dir, sink.as_ref(), llm.as_ref().map(|l| l as &dyn KindClassifier), apply)?;
    let report_path = match report {
        Some(p) => p.to_path_buf(),
        None => project.join(".context").join("memory-import").join(&date).join("report.json"),
    };
    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&report_path, serde_json::to_string_pretty(&records)?)?;
    let summary = summary_markdown(&records, apply);
    let summary_path = report_path.with_file_name("summary.md");
    std::fs::write(&summary_path, &summary)?;
    println!("{summary}");
    println!("report: {}\nsummary: {}", report_path.display(), summary_path.display());
    Ok(())
}

/// `path`, or `path-2`, `path-3`... if it already exists.
fn unique_dir(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    (2..).map(|n| PathBuf::from(format!("{}-{n}", path.display()))).find(|p| !p.exists()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_reads_top_level_and_nested_type_and_flags_a_malformed_file() {
        let nested = parse_entry("feedback_x.md", "---\nname: rule x\ndescription: \"one line\"\nmetadata:\n  node_type: memory\n  type: feedback\n  modified: 2026-09-28T17:34:46Z\n---\n\nBody.\n");
        assert_eq!((nested.name.as_deref(), nested.description.as_deref(), nested.claude_type.as_deref(), nested.modified.as_deref(), nested.body.as_str()), (Some("rule x"), Some("one line"), Some("feedback"), Some("2026-09-28T17:34:46Z"), "Body."));
        let top = parse_entry("a.md", "---\nname: a\ntype: project\n---\nBody\n");
        assert_eq!(top.claude_type.as_deref(), Some("project"));
        let broken = parse_entry("b.md", "---\nname: b\nno closing marker\n");
        assert!(broken.warnings.iter().any(|w| w.contains("malformed")) && broken.name.is_none());
    }

    #[test]
    fn the_index_maps_linked_files_to_their_line() {
        let index = index_lines("# Memory\n- [A](a.md) — hook\n- [B](b.md)\n");
        assert_eq!((index.get("a.md"), index.get("b.md"), index.get("c.md")), (Some(&2), Some(&3), None));
    }

    #[test]
    fn classification_order_is_frontmatter_then_prefix_then_heuristic_then_needs_review() {
        let e = |file: &str, t: Option<&str>, body: &str| Entry { file: file.into(), claude_type: t.map(String::from), body: body.into(), ..Default::default() };
        assert_eq!(classify(&e("project_a.md", Some("feedback"), "x"), None).unwrap().memory_kind, Some(MemoryKind::Procedural), "frontmatter beats the prefix");
        assert_eq!(classify(&e("user_a.md", None, "x"), None).unwrap().signal, "prefix");
        let heuristic = classify(&e("misc.md", None, "This is a known issue: the build silently fails."), None).unwrap();
        assert_eq!((heuristic.signal, heuristic.note_type), ("heuristic", Some(NoteType::Gotcha)));
        let none = classify(&e("misc.md", None, "Lunch is at noon."), None).unwrap();
        assert_eq!((none.signal, none.needs_review), ("none", Some(UNCLASSIFIED_REASON)));
        let stale = classify(&e("project_b.md", Some("project"), "Superseded by the new deploy flow."), None).unwrap();
        assert_eq!((stale.memory_kind, stale.needs_review), (Some(MemoryKind::Episodic), Some(STALE_REASON)));
    }

    struct Fixed(Option<MemoryKind>, f32);
    impl KindClassifier for Fixed {
        fn classify(&self, _: &str, _: &str) -> Result<(Option<MemoryKind>, f32)> {
            Ok((self.0, self.1))
        }
    }

    #[test]
    fn an_llm_verdict_below_the_confidence_floor_goes_to_needs_review() {
        let e = Entry { file: "misc.md".into(), body: "something".into(), ..Default::default() };
        assert_eq!(classify(&e, Some(&Fixed(Some(MemoryKind::Episodic), 0.9))).unwrap().memory_kind, Some(MemoryKind::Episodic));
        assert_eq!(classify(&e, Some(&Fixed(Some(MemoryKind::Episodic), 0.5))).unwrap().needs_review, Some(UNCLASSIFIED_REASON));
    }

    #[test]
    fn utc_date_formats_known_days() {
        assert_eq!(utc_date(std::time::UNIX_EPOCH), "1970-01-01");
        assert_eq!(utc_date(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_760_572_800)), "2025-10-16");
    }

    #[test]
    fn the_add_note_result_line_parses() {
        let out = parse_add_note_result("Dry run: would write x.\nresult: {\"note_type\":\"gotcha\",\"node_id\":null,\"written\":false,\"already_exists\":false,\"duplicate_of\":{\"node_id\":7,\"score\":0.95},\"similar_to\":[{\"node_id\":7,\"score\":0.95},{\"node_id\":9,\"score\":0.8}]}").unwrap();
        assert_eq!(out, NoteOutcome { note_type: "gotcha".into(), node_id: None, written: false, already_exists: false, duplicate_of: Some(7), similar_to: vec![7, 9] });
        assert!(parse_add_note_result("Wrote x.").is_err());
    }

    /// The spec's fixture: every signal type, an unclassifiable entry, a
    /// near-duplicate, a malformed file, a stale entry and an unindexed
    /// file. Entries in equals records out; the dry run writes nothing;
    /// `--apply` twice creates nodes once; the originals are untouched.
    #[test]
    fn a_fixture_memory_directory_imports_once_and_never_touches_the_originals() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("deploy.py"), "def deploy():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(repo.path(), false).unwrap();
        let memory = tempfile::tempdir().unwrap();
        let files: &[(&str, &str)] = &[
            ("feedback_tests.md", "---\nname: run tests on the server\ndescription: always run the suites on the server\nmetadata:\n  type: feedback\n---\n\nThe laptop build is too slow; use agentops-test-remote.\n"),
            ("project_deploy.md", "---\nname: deploy pipeline shipped\ntype: project\n---\n\nPush to main runs the CI gate and autodeploys the app container.\n"),
            ("project_deploy_copy.md", "---\nname: deploy pipeline is live\ntype: project\n---\n\nPush to main runs the CI gate and autodeploys the app container.\n"),
            ("reference_dashboard.md", "The Kuma dashboard on port 3001 shows the deploy heartbeat.\n"),
            ("user_role.md", "---\nname: user role\nmetadata:\n  type: user\n---\n\nThe user is a senior engineer who prefers terse answers.\n"),
            ("notes_misc.md", "Lunch with the team on Friday.\n"),
            ("broken.md", "---\nname: broken\nthis frontmatter never closes\n"),
            ("project_old.md", "---\nname: old topology\ntype: project\n---\n\nSuperseded: the two-backend setup was merged into one server.\n"),
        ];
        for (name, content) in files {
            std::fs::write(memory.path().join(name), content).unwrap();
        }
        let index: String = files.iter().filter(|(n, _)| *n != "user_role.md").map(|(n, _)| format!("- [{n}]({n})\n")).collect();
        std::fs::write(memory.path().join("MEMORY.md"), format!("# Memory\n{index}")).unwrap();
        let originals: Vec<Vec<u8>> = files.iter().map(|(n, _)| std::fs::read(memory.path().join(n)).unwrap()).collect();

        let sink = LocalSink { repo: repo.path().to_path_buf() };
        let store = agentops_mcp::open_store(repo.path()).unwrap();
        let name = agentops_mcp::repo_name(repo.path());
        let memories = || agentops_graph::similarity::MEMORY_KINDS.iter().map(|&k| store.nodes_by_kind(&name, k).unwrap().len()).sum::<usize>();

        let dry = import(memory.path(), &sink, None, false).unwrap();
        assert_eq!(dry.len(), files.len(), "entries in equals records out");
        assert_eq!(memories(), 0, "the dry run writes nothing to the graph");
        let by_file = |records: &[Record], f: &str| records.iter().find(|r| r.source_file == f).unwrap().clone();
        assert_eq!(by_file(&dry, "feedback_tests.md").memory_kind, Some(MemoryKind::Procedural));
        assert_eq!(by_file(&dry, "reference_dashboard.md").signal, "prefix");
        assert_eq!(by_file(&dry, "notes_misc.md").action, "needs_review");
        assert_eq!(by_file(&dry, "project_old.md").action, "needs_review");
        assert!(by_file(&dry, "broken.md").warnings.iter().any(|w| w.contains("malformed")));
        assert!(by_file(&dry, "user_role.md").source_line.is_none() && by_file(&dry, "user_role.md").warnings.iter().any(|w| w.contains("not linked")));

        let first = import(memory.path(), &sink, None, true).unwrap();
        assert_eq!(first.len(), files.len());
        let copy = by_file(&first, "project_deploy_copy.md");
        let original = by_file(&first, "project_deploy.md");
        assert_eq!((copy.action, copy.duplicate_of), ("duplicate", original.node_id), "{first:#?}");
        let created = memories();
        assert_eq!(created, files.len() - 1, "every entry but the duplicate becomes a node");
        let review = store.get_node(&name, by_file(&first, "notes_misc.md").node_id.unwrap()).unwrap().unwrap();
        assert_eq!(review.prominence, agentops_graph::NodeProminence::NeedsReview);
        assert!(agentops_notes::tags_of(review.content.as_deref().unwrap()).contains(&IMPORTED_TAG.to_string()));

        let second = import(memory.path(), &sink, None, true).unwrap();
        assert_eq!(memories(), created, "--apply twice creates nodes once");
        assert!(second.iter().all(|r| matches!(r.action, "skip" | "duplicate")), "{second:#?}");
        for ((n, _), bytes) in files.iter().zip(&originals) {
            assert_eq!(&std::fs::read(memory.path().join(n)).unwrap(), bytes, "{n} must be byte-identical");
        }

        // The command itself: backup before writing, report and summary beside it.
        run(repo.path(), Some(memory.path()), true, None, false).unwrap();
        let date = utc_date(std::time::SystemTime::now());
        let backup = repo.path().join(".context").join("memory-backup").join(&date);
        assert_eq!(std::fs::read(backup.join("feedback_tests.md")).unwrap(), originals[0], "the backup exists");
        let report_dir = repo.path().join(".context").join("memory-import").join(&date);
        let records: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(report_dir.join("report.json")).unwrap()).unwrap();
        assert_eq!(records.len(), files.len());
        let summary = std::fs::read_to_string(report_dir.join("summary.md")).unwrap();
        assert!(summary.contains("8 entries") && !summary.contains("Lunch with the team"), "counts, never entry text: {summary}");
    }
}
