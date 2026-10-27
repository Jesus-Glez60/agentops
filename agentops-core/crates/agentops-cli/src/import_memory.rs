//! `agentops import-memory`: imports a Claude Code auto-memory directory
//! (MEMORY.md plus one file per entry) into AgentOps (milestone 2 of the
//! cue-driven memory work). Without `--apply` it changes nothing and only
//! writes the report. Entries written directly in MEMORY.md, one top-level
//! bullet each, are imported too (milestone 5), with the same records.
//!
//! Every entry is classified into a memory kind, first match wins:
//! frontmatter `type` (top level or under `metadata`), then the file-name
//! prefix, then the heuristic classifier or `--llm-classify` at
//! `CLASSIFY_MIN_CONFIDENCE`, else `NeedsReview` ("unclassified on
//! import"). An entry whose description or MEMORY.md index line says it's
//! stale or superseded also goes to `NeedsReview`, except a rule: rules are
//! always pinned, never left waiting. Every entry gets one report record: entries in equals
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
/// A rule's first line: what the pin block shows, so it must read as one line.
pub(crate) const RULE_LINE_MAX: usize = 160;
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
    /// This file's line in MEMORY.md: the person's own one-line summary.
    index_line: Option<String>,
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

/// `MEMORY.md`'s `- [Title](file.md)` lines: file name → (line number, line).
fn index_lines(memory_md: &str) -> HashMap<String, (usize, String)> {
    memory_md
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let start = line.find("](")? + 2;
            let end = line[start..].find(')')? + start;
            Some((line[start..end].to_string(), (i + 1, line.to_string())))
        })
        .collect()
}

/// A `- [Title](file.md)` index line: a link to an entry file.
fn is_index_link(line: &str) -> bool {
    let Some(start) = line.find("](").map(|i| i + 2) else { return false };
    line[start..].find(')').is_some_and(|end| {
        let target = &line[start..start + end];
        target.ends_with(".md") && !target.contains("://")
    })
}

/// Entries written directly in MEMORY.md: each top-level bullet that isn't
/// an index link, with its indented sub-bullets as part of the body and
/// the nearest `##` heading as context. Index lines' own sub-bullets
/// describe the linked file and are not entries. Returns the entries
/// (with their line number) and the lines that are neither: prose a
/// person should look at, counted rather than dropped silently.
fn inline_entries(memory_md: &str) -> (Vec<(usize, Entry)>, usize) {
    let mut entries: Vec<(usize, Entry)> = Vec::new();
    let mut unparsed = 0;
    let mut heading: Option<String> = None;
    // The open bullet: its line and lines, or `None` under an index line.
    let mut open: Option<(usize, Vec<String>)> = None;
    let mut under_index = false;
    let close = |open: &mut Option<(usize, Vec<String>)>, heading: &Option<String>, entries: &mut Vec<(usize, Entry)>| {
        if let Some((n, lines)) = open.take() {
            let mut body = lines.join("\n");
            if let Some(h) = heading {
                body.push_str(&format!("\n\nContext: {h}"));
            }
            let first = lines.first().cloned().unwrap_or_default();
            entries.push((n, Entry { file: format!("MEMORY.md#L{n}"), index_line: Some(first), body, ..Default::default() }));
        }
    };
    for (i, line) in memory_md.lines().enumerate() {
        let n = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            close(&mut open, &heading, &mut entries);
            under_index = is_index_link(line);
            if !under_index {
                open = Some((n, vec![text.trim().to_string()]));
            }
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            match &mut open {
                Some((_, lines)) => lines.push(line.trim().to_string()),
                None if under_index => {}
                None => unparsed += 1,
            }
            continue;
        }
        close(&mut open, &heading, &mut entries);
        under_index = false;
        if let Some(h) = line.strip_prefix("## ") {
            heading = Some(h.trim().to_string());
        } else if !line.starts_with('#') {
            unparsed += 1;
        }
    }
    close(&mut open, &heading, &mut entries);
    (entries, unparsed)
}

/// Entries MEMORY.md and its directory should yield, counted by line type
/// independently of the parsers: each entry file, and each top-level
/// bullet that isn't an index link. Records out must equal this.
fn expected_entries(memory_dir: &Path) -> Result<usize> {
    let files = std::fs::read_dir(memory_dir)?.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "md") && e.file_name() != "MEMORY.md").count();
    let md = std::fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap_or_default();
    let bullets = md.lines().filter(|l| (l.starts_with("- ") || l.starts_with("* ")) && !is_index_link(l)).count();
    Ok(files + bullets)
}

/// An inline entry's title: its first sentence.
fn inline_title(first_line: &str) -> Option<String> {
    let sentence = first_line.split_once(". ").map_or(first_line, |(s, _)| s);
    description_title(sentence.trim_start_matches("**").replace("**", "").trim())
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
    // Only the entry's own one-line summaries count: its description and its
    // MEMORY.md line. Bodies use these words in other senses ("a stale
    // browser tab", "possibly stale" notes): checking them sent a quarter
    // of the agentops pilot's entries to review (2026-10-17 dry run).
    let stale = {
        let text = format!("{} {}", entry.description.as_deref().unwrap_or(""), entry.index_line.as_deref().unwrap_or("")).to_lowercase();
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
    if stale && classified.needs_review.is_none() && classified.memory_kind != Some(MemoryKind::Procedural) {
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
    /// Renames a memory in place (`agentops_mcp::retitle_note`); false
    /// when it already had this title.
    fn retitle(&self, node_id: i64, title: &str) -> Result<bool>;
}

pub(crate) struct LocalSink {
    pub(crate) repo: PathBuf,
}

impl Sink for LocalSink {
    fn add_note(&self, title: &str, body: &str, note_type: Option<NoteType>, tags: &[String], mode: AddMode) -> Result<NoteOutcome> {
        let r = agentops_mcp::add_note_with(&self.repo, title, body, note_type, tags, None, true, mode)?;
        // The remote sink's add_note classes the note server-side; the
        // local one does it here, the same way (ADR 0054).
        if let (true, Some(id)) = (r.written, r.node_id) {
            agentops_mcp::apply_memory_class(&self.repo, id, title, None, &[])?;
        }
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

    fn retitle(&self, node_id: i64, title: &str) -> Result<bool> {
        let store = agentops_mcp::open_store(&self.repo)?;
        agentops_mcp::retitle_note(store.as_ref(), &agentops_mcp::repo_name(&self.repo), &agentops_notes::resolve_notes_path(&self.repo, None), node_id, title)
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

    fn retitle(&self, node_id: i64, title: &str) -> Result<bool> {
        let url = format!("{}/repos/{}/nodes/{node_id}/title", self.marker.server_url.trim_end_matches('/'), self.marker.connection_id);
        let mut response = ureq::post(&url)
            .header("Authorization", &format!("Bearer {}", self.marker.api_key))
            .config()
            .http_status_as_error(false)
            .build()
            .send_json(json!({ "title": title }))
            .with_context(|| format!("calling POST {url}"))?;
        let reply: serde_json::Value = response.body_mut().read_json().unwrap_or_default();
        if !response.status().is_success() {
            anyhow::bail!("POST {url} returned {}: {}", response.status(), reply["error"].as_str().unwrap_or_default());
        }
        Ok(reply["changed"].as_bool().unwrap_or(true))
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
    /// For a pinned rule: the line its pin shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rule_line: Option<String>,
    /// Host names (this machine, `~/.ssh/config` aliases) the entry
    /// mentions: a scope proposal, never applied by the import.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) scope_hosts: Vec<String>,
}

/// The rule a procedural entry pins: its one-line description, else its
/// body's first line, cut to `RULE_LINE_MAX` characters.
fn rule_line(entry: &Entry) -> String {
    let source = entry.description.clone().unwrap_or_else(|| entry.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string());
    if source.chars().count() <= RULE_LINE_MAX {
        source
    } else {
        format!("{}…", source.chars().take(RULE_LINE_MAX - 1).collect::<String>().trim_end())
    }
}

/// The note text for an entry. A procedural entry is its rule line, then
/// the reasoning (pins show only the first line). Otherwise its one-line
/// description first (unless the body already starts with it), then the
/// body; an episodic entry keeps its date.
fn note_body(entry: &Entry, kind: Option<MemoryKind>, mtime_date: &str) -> String {
    if kind == Some(MemoryKind::Procedural) {
        return format!("{}\n\n{}", rule_line(entry), entry.body).trim().to_string();
    }
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

/// One entry to import: from an entry file or a MEMORY.md bullet.
struct Pending {
    entry: Entry,
    source_file: String,
    source_line: Option<usize>,
    title: String,
    /// The title when `title` is taken by an earlier entry.
    fallback_title: String,
    /// The title an earlier import may have used: an entry file's slug.
    /// An inline entry has none, so it's only ever found under its title.
    legacy_title: Option<String>,
    mtime_date: String,
}

/// What an import found besides its records.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct ImportCounts {
    pub(crate) expected: usize,
    /// MEMORY.md lines that are neither an entry, an index line nor a heading.
    pub(crate) unparsed_lines: usize,
}

/// Classifies and (with `apply`) writes every entry in `memory_dir`.
/// Returns one record per entry: entry files sorted by name, then the
/// bullets written in MEMORY.md in line order.
#[cfg(test)]
pub(crate) fn import(memory_dir: &Path, sink: &dyn Sink, llm: Option<&dyn KindClassifier>, apply: bool, retitle: bool) -> Result<Vec<Record>> {
    Ok(import_counted(memory_dir, sink, llm, apply, retitle, &[])?.0)
}

/// `import`, with the independent entry count and scope proposals for the
/// host `names` given.
pub(crate) fn import_counted(memory_dir: &Path, sink: &dyn Sink, llm: Option<&dyn KindClassifier>, apply: bool, retitle: bool, names: &[String]) -> Result<(Vec<Record>, ImportCounts)> {
    let memory_md = std::fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap_or_default();
    let index = index_lines(&memory_md);
    let mut files: Vec<PathBuf> = std::fs::read_dir(memory_dir)
        .with_context(|| format!("reading {}", memory_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md") && p.file_name().is_some_and(|n| n != "MEMORY.md"))
        .collect();
    files.sort();

    let mut records = Vec::new();
    let mut pending = Vec::new();
    for path in files {
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        let source_line = index.get(&file).map(|(line, _)| *line);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                records.push(skipped(&file, source_line, &file, vec![format!("unreadable: {e}")]));
                continue;
            }
        };
        let mut entry = parse_entry(&file, &content);
        entry.index_line = index.get(&file).map(|(_, text)| text.clone());
        if source_line.is_none() {
            entry.warnings.push("not linked from MEMORY.md".into());
        }
        // A memory's `name` is a file slug; its description reads as a
        // title, so that's the title (F55). Memories imported before carry
        // the slug, and are found under it below.
        let slug_title = entry.name.clone().unwrap_or_else(|| file.trim_end_matches(".md").to_string());
        let title = entry.description.as_deref().and_then(description_title).unwrap_or_else(|| slug_title.clone());
        let mtime_date = std::fs::metadata(&path).and_then(|m| m.modified()).map(utc_date).unwrap_or_default();
        pending.push(Pending { entry, source_file: file, source_line, title, fallback_title: slug_title.clone(), legacy_title: Some(slug_title), mtime_date });
    }
    let (inline, unparsed_lines) = inline_entries(&memory_md);
    let md_mtime = std::fs::metadata(memory_dir.join("MEMORY.md")).and_then(|m| m.modified()).map(utc_date).unwrap_or_default();
    for (n, entry) in inline {
        let title = entry.index_line.as_deref().and_then(inline_title).unwrap_or_else(|| format!("MEMORY.md line {n}"));
        pending.push(Pending { source_file: entry.file.clone(), source_line: Some(n), fallback_title: format!("{title} (MEMORY.md L{n})"), legacy_title: None, title, mtime_date: md_mtime.clone(), entry });
    }

    let mut titles_used = std::collections::HashSet::new();
    let mut slugs_used = std::collections::HashSet::new();
    for Pending { mut entry, source_file: file, source_line, mut title, fallback_title, legacy_title, mtime_date } in pending {
        // Two entries sharing a title would share a natural key, and the
        // second would read as already imported instead of a duplicate; two
        // whose titles differ only in punctuation would share a note file
        // (gotchas 195360, 387595).
        if !titles_used.insert(title.clone()) | !slugs_used.insert(agentops_mcp::slug(&title)) {
            entry.warnings.push(format!("title shared with an earlier entry; titled {fallback_title:?}"));
            title = fallback_title;
            titles_used.insert(title.clone());
            slugs_used.insert(agentops_mcp::slug(&title));
        }
        let slug_title = legacy_title.unwrap_or_else(|| title.clone());
        if entry.body.is_empty() && entry.description.is_none() {
            records.push(skipped(&file, source_line, &title, [entry.warnings, vec!["empty entry".into()]].concat()));
            continue;
        }
        let classified = classify(&entry, llm)?;
        let entry_for_rule = Entry { description: entry.description.clone(), body: entry.body.clone(), ..Default::default() };
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
        let under_slug = if !preview.already_exists && title != slug_title { Some(sink.add_note(&slug_title, &body, note_type, &tags, AddMode::DryRun)?).filter(|p| p.already_exists) } else { None };
        if let Some(old) = under_slug {
            let mut record = skipped(&file, source_line, &title, entry.warnings);
            record.node_id = old.node_id;
            record.node_kind = Some(old.note_type);
            if !retitle {
                record.warnings.push(format!("already imported as node {} under its slug title; --retitle renames it", old.node_id.unwrap_or_default()));
            } else {
                record.action = "retitle";
                record.warnings.push(format!("was {slug_title:?}"));
                if let (true, Some(id)) = (apply, old.node_id) {
                    sink.retitle(id, &title)?;
                }
            }
            records.push(record);
            continue;
        }
        if retitle && title == slug_title {
            entry.warnings.push("no description; title kept".into());
        }
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
            rule_line: None,
            scope_hosts: crate::classify_scope::mentioned(&format!("{title}\n{body}"), names),
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

        // A rule is pinned: always present, never left to recall. When it
        // nearly copies a memory already recorded, that memory is pinned
        // instead, so no rule is lost when MEMORY.md is turned off.
        let pin = classified.memory_kind == Some(MemoryKind::Procedural) && classified.needs_review.is_none();
        record.pinned = pin && matches!(record.action, "create" | "duplicate");
        if apply && matches!(record.action, "create" | "needs_review") {
            let written = sink.add_note(&title, &body, note_type, &tags, AddMode::WriteUnlessDuplicate)?;
            record.node_id = written.node_id;
            record.similar_to = written.similar_to;
            if !written.written {
                record.action = "duplicate";
                record.duplicate_of = written.duplicate_of;
            } else if let (Some(reason), Some(id)) = (classified.needs_review, written.node_id) {
                sink.set_curation(id, "needs_review", Some(reason))?;
            } else if let (true, Some(id)) = (pin, written.node_id) {
                sink.set_curation(id, "pinned", None)?;
            }
        }
        if apply && pin && record.action == "duplicate" {
            if let Some(existing) = record.duplicate_of {
                sink.set_curation(existing, "pinned", None)?;
                record.warnings.push(format!("rule pinned on the memory it duplicates (node {existing})"));
            }
        }
        record.rule_line = pin.then(|| rule_line(&entry_for_rule));
        records.push(record);
    }
    Ok((records, ImportCounts { expected: expected_entries(memory_dir)?, unparsed_lines }))
}

fn skipped(file: &str, source_line: Option<usize>, title: &str, warnings: Vec<String>) -> Record {
    Record { source_file: file.into(), source_line, title: title.into(), signal: "none", memory_kind: None, node_kind: None, confidence: None, action: "skip", node_id: None, duplicate_of: None, pinned: false, similar_to: Vec::new(), warnings, rule_line: None, scope_hosts: Vec::new() }
}

/// Longest title taken from a description. Longer ones are cut at the
/// last word boundary before it, with an ellipsis.
const RETITLE_MAX: usize = 90;

/// A description as a title: first line, trimmed, without a closing
/// period, cut to `RETITLE_MAX` at a word boundary. `None` when empty.
fn description_title(description: &str) -> Option<String> {
    let line = description.lines().next()?.trim().trim_end_matches('.').trim();
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= RETITLE_MAX {
        return Some(line.to_string());
    }
    let head: String = line.chars().take(RETITLE_MAX).collect();
    let cut = head.rfind(char::is_whitespace).map_or(head.as_str(), |i| &head[..i]);
    Some(format!("{}…", cut.trim_end_matches(|c: char| !c.is_alphanumeric())))
}

/// Counts and warnings, plus the pin block the imported rules add (their
/// rule lines, which the spec asks to see with its size). No other entry
/// text, so everything but the pin block can be quoted safely.
#[cfg(test)]
pub(crate) fn summary_markdown(records: &[Record], apply: bool) -> String {
    summary_markdown_counted(records, apply, None)
}

/// Most scope proposals a summary lists (gotcha 354625: name matching
/// over-proposes); the rest are counted.
const SCOPE_SHOWN_MAX: usize = 10;

/// `summary_markdown`, with the import's independent entry count.
pub(crate) fn summary_markdown_counted(records: &[Record], apply: bool, counts: Option<&ImportCounts>) -> String {
    let by = |f: &dyn Fn(&Record) -> String| -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for r in records {
            *m.entry(f(r)).or_insert(0) += 1;
        }
        m
    };
    let table = |title: &str, m: BTreeMap<String, usize>| format!("## {title}\n\n{}\n", m.iter().map(|(k, v)| format!("- {k}: {v}")).collect::<Vec<_>>().join("\n"));
    let entries = counts.map_or(records.len(), |c| c.expected);
    let mut out = format!("# MEMORY.md import ({})\n\n{entries} entries, {} records.\n\n", if apply { "applied" } else { "dry run" }, records.len());
    if entries != records.len() {
        out.push_str(&format!("**Entries in ({entries}) and records out ({}) differ: an entry was lost or doubled.**\n\n", records.len()));
    }
    if let Some(c) = counts.filter(|c| c.unparsed_lines > 0) {
        out.push_str(&format!("MEMORY.md has {} unparsed lines (prose that is neither an entry, an index line nor a heading); nothing was made of them.\n\n", c.unparsed_lines));
    }
    out.push_str(&table("By action", by(&|r| r.action.to_string())));
    out.push_str(&table("By memory kind", by(&|r| r.memory_kind.map_or("unclassified".into(), |k| k.as_str().to_string()))));
    out.push_str(&table("By signal", by(&|r| r.signal.to_string())));
    out.push_str(&table("By node kind", by(&|r| r.node_kind.clone().unwrap_or_else(|| "none".into()))));
    // The pin block these rules add (the spec's "rendered pin block with its
    // size"), in the hook's own format and budget. Pins already on the
    // server count against the same budget; the pilot measures the real one.
    let pins: Vec<String> = records.iter().filter(|r| r.pinned).filter_map(|r| r.rule_line.as_ref().map(|line| format!("- {line} (id {})", r.node_id.or(r.duplicate_of).map_or("new".into(), |id| id.to_string())))).collect();
    let block = format!("## Pinned\n{}\n", pins.join("\n"));
    let size = block.chars().count();
    let budget = agentops_mcp::budget::PIN_BUDGET;
    let mut used = "## Pinned\n".chars().count() + 160;
    let cut: Vec<&str> = records.iter().filter(|r| r.pinned).filter(|r| {
        used += r.rule_line.as_ref().map_or(0, |l| l.chars().count() + 12);
        used > budget
    }).map(|r| r.source_file.as_str()).collect();
    out.push_str(&format!("\n## Pinned rules ({} rules, {size} characters, budget {budget})\n\n```\n{block}```\n", pins.len()));
    if !cut.is_empty() {
        out.push_str(&format!("\n**Over the pin budget: these would be cut** (named in the pin block, never dropped silently): {}\n", cut.join(", ")));
    }
    let mut scoped: Vec<&Record> = records.iter().filter(|r| !r.scope_hosts.is_empty()).collect();
    scoped.sort_by(|a, b| b.scope_hosts.len().cmp(&a.scope_hosts.len()).then(a.source_file.cmp(&b.source_file)));
    out.push_str(&format!("\n## Scope proposals ({} of {}, never applied)\n\n", scoped.len().min(SCOPE_SHOWN_MAX), scoped.len()));
    for r in scoped.iter().take(SCOPE_SHOWN_MAX) {
        out.push_str(&format!("- `{}`: {}\n", r.source_file, r.scope_hosts.join(", ")));
    }
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
pub(crate) fn run(project: &Path, memory_dir: Option<&Path>, apply: bool, report: Option<&Path>, llm_classify: bool, retitle: bool, backup_dir: Option<&Path>) -> Result<()> {
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
        let backup = unique_dir(&backup_dir.map(Path::to_path_buf).unwrap_or_else(|| project.join(".context").join("memory-backup").join(&date)));
        std::fs::create_dir_all(&backup)?;
        for entry in std::fs::read_dir(&memory_dir)? {
            let path = entry?.path();
            if path.is_file() {
                std::fs::copy(&path, backup.join(path.file_name().unwrap()))?;
            }
        }
        println!("backed up {} to {}", memory_dir.display(), backup.display());
    }

    let (_, names) = crate::classify_scope::local_candidate_names(&[]);
    let (records, counts) = import_counted(&memory_dir, sink.as_ref(), llm.as_ref().map(|l| l as &dyn KindClassifier), apply, retitle, &names)?;
    let report_path = match report {
        Some(p) => p.to_path_buf(),
        None => project.join(".context").join("memory-import").join(&date).join("report.json"),
    };
    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&report_path, serde_json::to_string_pretty(&records)?)?;
    let summary = summary_markdown_counted(&records, apply, Some(&counts));
    let summary_path = report_path.with_file_name("summary.md");
    std::fs::write(&summary_path, &summary)?;
    println!("{summary}");
    println!("report: {}\nsummary: {}", report_path.display(), summary_path.display());
    anyhow::ensure!(counts.expected == records.len(), "{} entries but {} records: see {}", counts.expected, records.len(), summary_path.display());
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
    fn a_description_becomes_the_title_cut_at_a_word_boundary() {
        assert_eq!(description_title("Deploys go through push-to-main.\nmore"), Some("Deploys go through push-to-main".into()));
        assert_eq!(description_title("   "), None);
        let long = "word ".repeat(40);
        let title = description_title(&long).unwrap();
        assert!(title.ends_with("word…") && title.chars().count() <= RETITLE_MAX + 1, "{title}");
    }

    #[test]
    fn an_imported_body_starts_with_its_description_so_a_retitle_never_needs_a_new_vector() {
        // Retitling keeps the vector: notes are embedded from their body,
        // and an imported (non-rule) body leads with the description.
        let entry = Entry { description: Some("the summary line".into()), body: "Details.".into(), ..Default::default() };
        for kind in [None, Some(MemoryKind::Semantic), Some(MemoryKind::Episodic)] {
            assert!(note_body(&entry, kind, "2026-10-19").starts_with("the summary line"), "{kind:?}");
        }
    }

    #[test]
    fn memories_imported_under_slug_titles_are_found_retitled_with_retitle_and_never_duplicated() {
        let repo = tempfile::tempdir().unwrap();
        agentops_mcp::scan_and_persist(repo.path(), false).unwrap();
        let memory = tempfile::tempdir().unwrap();
        let content = "---\nname: project-deploy-pipeline\ndescription: Deploys go through the push-to-main gate\ntype: project\n---\n\nPush to main runs the CI gate.\n";
        std::fs::write(memory.path().join("project_deploy.md"), content).unwrap();
        std::fs::write(memory.path().join("MEMORY.md"), "# Memory\n- [d](project_deploy.md)\n").unwrap();
        let sink = LocalSink { repo: repo.path().to_path_buf() };
        let store = agentops_mcp::open_store(repo.path()).unwrap();
        let name = agentops_mcp::repo_name(repo.path());
        // As milestone 2 imported it: titled by the slug.
        let entry = parse_entry("project_deploy.md", content);
        let body = note_body(&entry, Some(MemoryKind::Episodic), "2026-10-17");
        let tags = vec![IMPORTED_TAG.to_string(), "memory-kind:episodic".to_string()];
        let old = sink.add_note("project-deploy-pipeline", &body, Some(NoteType::Knowledge), &tags, AddMode::Write).unwrap().node_id.unwrap();
        let count = || store.nodes_by_kind(&name, agentops_graph::NodeKind::Note).unwrap().len();
        assert_eq!(count(), 1);

        let plain = import(memory.path(), &sink, None, true, false).unwrap();
        assert_eq!((plain[0].action, plain[0].node_id), ("skip", Some(old)), "{plain:?}");
        assert!(plain[0].warnings.iter().any(|w| w.contains("--retitle")), "{plain:?}");
        let dry = import(memory.path(), &sink, None, false, true).unwrap();
        assert_eq!(dry[0].action, "retitle");
        assert_eq!(store.get_node(&name, old).unwrap().unwrap().name.as_deref(), Some("project-deploy-pipeline"), "a dry run renames nothing");

        let applied = import(memory.path(), &sink, None, true, true).unwrap();
        assert_eq!((applied[0].action, applied[0].title.as_str()), ("retitle", "Deploys go through the push-to-main gate"));
        assert_eq!(store.get_node(&name, old).unwrap().unwrap().name.as_deref(), Some("Deploys go through the push-to-main gate"));
        let again = import(memory.path(), &sink, None, true, true).unwrap();
        assert_eq!((again[0].action, again[0].node_id), ("skip", Some(old)), "{again:?}");
        assert_eq!(count(), 1, "the importer still finds the note after a rename");
    }

    #[test]
    fn the_index_maps_linked_files_to_their_line() {
        let index = index_lines("# Memory\n- [A](a.md) — hook\n- [B](b.md)\n");
        assert_eq!((index.get("a.md").map(|l| l.0), index.get("b.md").map(|l| l.0), index.get("c.md")), (Some(2), Some(3), None));
        assert_eq!(index["a.md"].1, "- [A](a.md) — hook");
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
        let stale_body = classify(&e("project_b.md", Some("project"), "Superseded by the new deploy flow."), None).unwrap();
        assert_eq!(stale_body.needs_review, None, "only the one-line summaries count, not the body");
        let stale = classify(&Entry { description: Some("old topology, superseded".into()), ..e("project_b.md", Some("project"), "x") }, None).unwrap();
        assert_eq!((stale.memory_kind, stale.needs_review), (Some(MemoryKind::Episodic), Some(STALE_REASON)));
        let by_index = classify(&Entry { index_line: Some("- [B](b.md) — STALE stub note".into()), ..e("project_b.md", Some("project"), "x") }, None).unwrap();
        assert_eq!(by_index.needs_review, Some(STALE_REASON));
        let rule = classify(&Entry { description: Some("a stale tab causes a false alarm; restart first".into()), ..e("feedback_b.md", Some("feedback"), "x") }, None).unwrap();
        assert_eq!((rule.memory_kind, rule.needs_review), (Some(MemoryKind::Procedural), None), "a rule is never sent to review");
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
    fn a_rule_line_is_the_description_or_first_line_cut_to_the_max() {
        let with_description = Entry { description: Some("never push without asking".into()), body: "Because a push deploys.".into(), ..Default::default() };
        assert_eq!(rule_line(&with_description), "never push without asking");
        let long = Entry { body: format!("{}\nreasoning", "y".repeat(400)), ..Default::default() };
        let line = rule_line(&long);
        assert!(line.chars().count() <= RULE_LINE_MAX && line.ends_with('…'));
        assert_eq!(note_body(&with_description, Some(MemoryKind::Procedural), "2026-10-16"), "never push without asking\n\nBecause a push deploys.");
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
    fn inline_bullets_are_entries_with_their_sub_bullets_and_heading_and_prose_is_counted() {
        let md = "# Memory\n\nSome intro prose.\n\n## Deploys\n- Deploys go through push-to-main. The gate runs first.\n  - rollbacks are automatic\n- [Linked](linked_file.md)\n  - describes the linked file\n* Never force-push main\n\n## Data\n- The pool runs out at 50 clients\n";
        let (entries, unparsed) = inline_entries(md);
        assert_eq!(unparsed, 1, "the intro line");
        let lines: Vec<usize> = entries.iter().map(|(n, _)| *n).collect();
        assert_eq!(lines, vec![6, 10, 13]);
        assert_eq!(entries[0].1.body, "Deploys go through push-to-main. The gate runs first.\n- rollbacks are automatic\n\nContext: Deploys");
        assert_eq!(entries[0].1.file, "MEMORY.md#L6");
        assert!(entries.iter().all(|(_, e)| !e.body.contains("describes the linked file")), "an index line's notes aren't an entry");
        assert_eq!(inline_title("Deploys go through push-to-main. The gate runs first."), Some("Deploys go through push-to-main".into()));
        assert_eq!(inline_title("**Bold** rule here"), Some("Bold rule here".into()));
        let (none, _) = inline_entries("- a fact with no heading\n");
        assert_eq!(none[0].1.body, "a fact with no heading");
    }

    #[test]
    fn an_inline_memory_md_imports_entry_by_entry_once_with_shared_titles_kept_apart() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("app.py"), "def app():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(repo.path(), false).unwrap();
        let memory = tempfile::tempdir().unwrap();
        std::fs::write(
            memory.path().join("MEMORY.md"),
            "# Memory\n\n## Rules\n- Never push to main without the gate\n- Never push to main without the gate. Second copy, other reason: releases.\n- Never push to main, without the gate!\n\n## Facts\n- The pool runs out at 50 clients under load.\nA stray paragraph.\n- [file](entry.md)\n",
        )
        .unwrap();
        std::fs::write(memory.path().join("entry.md"), "---\nname: entry\ndescription: a linked entry file\ntype: project\n---\n\nIt happened.\n").unwrap();
        let sink = LocalSink { repo: repo.path().to_path_buf() };
        let (dry, counts) = import_counted(memory.path(), &sink, None, false, false, &["thedamnserver".to_string()]).unwrap();
        assert_eq!((dry.len(), counts.expected, counts.unparsed_lines), (5, 5, 1), "{dry:#?}");
        let titles: Vec<&str> = dry.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles.iter().collect::<std::collections::HashSet<_>>().len(), 5, "every title distinct: {titles:?}");
        let slugs: std::collections::HashSet<String> = titles.iter().map(|t| agentops_mcp::slug(t)).collect();
        assert_eq!(slugs.len(), 5, "no two share a note file: {titles:?}");
        assert!(dry.iter().filter(|r| r.source_file.starts_with("MEMORY.md#L")).all(|r| r.signal != "frontmatter" && r.source_line.is_some()));

        let store = agentops_mcp::open_store(repo.path()).unwrap();
        let name = agentops_mcp::repo_name(repo.path());
        let memories = || agentops_graph::similarity::MEMORY_KINDS.iter().map(|&k| store.nodes_by_kind(&name, k).unwrap().len()).sum::<usize>();
        let (first, _) = import_counted(memory.path(), &sink, None, true, false, &[]).unwrap();
        let created = memories();
        assert_eq!(created, first.iter().filter(|r| r.action != "duplicate").count(), "{first:#?}");
        let (second, _) = import_counted(memory.path(), &sink, None, true, false, &[]).unwrap();
        assert_eq!(memories(), created, "a re-run creates nothing");
        assert!(second.iter().all(|r| matches!(r.action, "skip" | "duplicate")), "{second:#?}");
        let summary = summary_markdown_counted(&second, true, Some(&counts));
        assert!(summary.contains("5 entries, 5 records") && summary.contains("1 unparsed lines") && !summary.contains("50 clients"), "{summary}");
    }

    #[test]
    fn a_scope_proposal_names_the_hosts_an_entry_mentions_and_is_never_applied() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("app.py"), "def app():\n    pass\n").unwrap();
        agentops_mcp::scan_and_persist(repo.path(), false).unwrap();
        let memory = tempfile::tempdir().unwrap();
        std::fs::write(memory.path().join("MEMORY.md"), "- The CI runner on build-box needs docker\n- Unrelated fact about colors\n").unwrap();
        let sink = LocalSink { repo: repo.path().to_path_buf() };
        let (records, counts) = import_counted(memory.path(), &sink, None, false, false, &["build-box".to_string()]).unwrap();
        assert_eq!(records[0].scope_hosts, vec!["build-box".to_string()]);
        assert!(records[1].scope_hosts.is_empty());
        let summary = summary_markdown_counted(&records, false, Some(&counts));
        assert!(summary.contains("## Scope proposals (1 of 1, never applied)") && summary.contains("`MEMORY.md#L1`: build-box"), "{summary}");
    }

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
            ("project_old.md", "---\nname: old topology\ndescription: superseded two-backend setup\ntype: project\n---\n\nThe two-backend setup was merged into one server.\n"),
            ("feedback_tests_again.md", "---\nname: tests belong on the server\ndescription: always run the suites on the server\nmetadata:\n  type: feedback\n---\n\nThe laptop build is too slow; use agentops-test-remote.\n"),
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

        let dry = import(memory.path(), &sink, None, false, false).unwrap();
        assert_eq!(dry.len(), files.len(), "entries in equals records out");
        assert_eq!(memories(), 0, "the dry run writes nothing to the graph");
        let by_file = |records: &[Record], f: &str| records.iter().find(|r| r.source_file == f).unwrap().clone();
        assert_eq!(by_file(&dry, "feedback_tests.md").memory_kind, Some(MemoryKind::Procedural));
        assert_eq!(by_file(&dry, "reference_dashboard.md").signal, "prefix");
        assert_eq!(by_file(&dry, "notes_misc.md").action, "needs_review");
        assert_eq!(by_file(&dry, "project_old.md").action, "needs_review");
        assert!(by_file(&dry, "broken.md").warnings.iter().any(|w| w.contains("malformed")));
        assert!(by_file(&dry, "user_role.md").source_line.is_none() && by_file(&dry, "user_role.md").warnings.iter().any(|w| w.contains("not linked")));

        let first = import(memory.path(), &sink, None, true, false).unwrap();
        assert_eq!(first.len(), files.len());
        let copy = by_file(&first, "project_deploy_copy.md");
        let original = by_file(&first, "project_deploy.md");
        assert_eq!((copy.action, copy.duplicate_of), ("duplicate", original.node_id), "{first:#?}");
        let created = memories();
        assert_eq!(created, files.len() - 2, "every entry but the two duplicates becomes a node");
        let again_rule = by_file(&first, "feedback_tests_again.md");
        let first_rule = by_file(&first, "feedback_tests.md");
        assert_eq!((again_rule.action, again_rule.duplicate_of, again_rule.pinned), ("duplicate", first_rule.node_id, true), "a duplicate rule pins the memory it copies");
        let rule = by_file(&first, "feedback_tests.md");
        assert!(rule.pinned && rule.rule_line.as_deref() == Some("always run the suites on the server"), "{rule:?}");
        let rule_node = store.get_node(&name, rule.node_id.unwrap()).unwrap().unwrap();
        assert_eq!(rule_node.prominence, agentops_graph::NodeProminence::Pinned);
        assert_eq!(rule_node.content.as_deref().unwrap().lines().next(), Some("always run the suites on the server"), "the rule is the first line");
        let (block, cut) = agentops_mcp::session_guide::pinned_block(store.as_ref(), &name).unwrap();
        assert!(block.contains(&format!("- always run the suites on the server (id {})", rule_node.id)) && cut.is_empty(), "{block}");
        assert!(!by_file(&first, "project_deploy.md").pinned, "only rules are pinned");
        let review = store.get_node(&name, by_file(&first, "notes_misc.md").node_id.unwrap()).unwrap().unwrap();
        assert_eq!(review.prominence, agentops_graph::NodeProminence::NeedsReview);
        assert!(agentops_notes::tags_of(review.content.as_deref().unwrap()).contains(&IMPORTED_TAG.to_string()));

        let second = import(memory.path(), &sink, None, true, false).unwrap();
        assert_eq!(memories(), created, "--apply twice creates nodes once");
        assert!(second.iter().all(|r| matches!(r.action, "skip" | "duplicate")), "{second:#?}");
        for ((n, _), bytes) in files.iter().zip(&originals) {
            assert_eq!(&std::fs::read(memory.path().join(n)).unwrap(), bytes, "{n} must be byte-identical");
        }

        // The command itself: backup before writing, report and summary beside it.
        run(repo.path(), Some(memory.path()), true, None, false, false, None).unwrap();
        let date = utc_date(std::time::SystemTime::now());
        let backup = repo.path().join(".context").join("memory-backup").join(&date);
        assert_eq!(std::fs::read(backup.join("feedback_tests.md")).unwrap(), originals[0], "the backup exists");
        let report_dir = repo.path().join(".context").join("memory-import").join(&date);
        let records: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(report_dir.join("report.json")).unwrap()).unwrap();
        assert_eq!(records.len(), files.len());
        let summary = std::fs::read_to_string(report_dir.join("summary.md")).unwrap();
        assert!(summary.contains("9 entries") && !summary.contains("Lunch with the team"), "counts, never entry text: {summary}");
        assert!(summary.contains("## Pinned rules (1 rules"), "a re-run only re-pins the duplicate rule's existing memory: {summary}");
        let first_summary = summary_markdown(&first, true);
        assert!(first_summary.contains("## Pinned rules (2 rules") && first_summary.contains("- always run the suites on the server (id "), "{first_summary}");
    }
}
