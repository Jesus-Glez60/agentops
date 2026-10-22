//! `agentops classify-memories --remote [--apply]` (milestone 3b revision
//! 2, ADR 0054): gives the repo's existing gotchas, decisions and notes a
//! memory class. The dry run (default) pages through the server's
//! `memories/classify`, which writes nothing, and saves its proposals to
//! `.context/classify-memories.{json,md}` for the author to read. `--apply`
//! then sends exactly those saved proposals to `memories/class`, so what
//! gets written is what was reviewed, never a fresh LLM judgment.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::recall_eval::remote_send_within;
use crate::remote_marker::read_remote_marker;

/// One classify page judges up to 50 memories, each maybe one LLM call.
const PAGE_TIMEOUT: Duration = Duration::from_secs(600);
/// Assignments sent per write request.
const ASSIGN_CHUNK: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Proposal {
    node_id: i64,
    kind: String,
    title: String,
    current: String,
    memory_class: String,
    #[serde(default)]
    class_until: Option<String>,
}

impl Proposal {
    fn changes(&self) -> bool {
        self.memory_class != self.current || self.class_until.is_some()
    }
}

#[derive(Deserialize)]
struct Page {
    total: usize,
    judge: String,
    proposals: Vec<Proposal>,
}

pub(crate) fn run(path: &Path, apply: bool) -> Result<()> {
    let marker = read_remote_marker(path).with_context(|| format!("classify-memories runs against the server; {} has no .context/agentops-remote.json", path.display()))?;
    let dir = path.join(".context");
    let saved = dir.join("classify-memories.json");

    if apply {
        let proposals: Vec<Proposal> = serde_json::from_str(&std::fs::read_to_string(&saved).with_context(|| format!("run the dry run first; no {}", saved.display()))?)?;
        let changes: Vec<&Proposal> = proposals.iter().filter(|p| p.changes()).collect();
        let mut assigned = 0;
        for chunk in changes.chunks(ASSIGN_CHUNK) {
            let assign: Vec<_> = chunk.iter().map(|p| json!({ "node_id": p.node_id, "memory_class": p.memory_class, "class_until": p.class_until })).collect();
            let reply = remote_send_within(&marker, "memories/class", Some(&json!({ "assign": assign })), PAGE_TIMEOUT)?;
            assigned += reply["assigned"].as_u64().unwrap_or(0) as usize;
        }
        println!("Assigned {assigned} of {} reviewed classes from {}.", changes.len(), saved.display());
        return Ok(());
    }

    let mut proposals = Vec::new();
    let mut judge;
    loop {
        let page: Page = serde_json::from_value(remote_send_within(&marker, "memories/classify", Some(&json!({ "offset": proposals.len() })), PAGE_TIMEOUT)?)?;
        judge = page.judge;
        let got = page.proposals.len();
        proposals.extend(page.proposals);
        eprintln!("classified {} of {}", proposals.len(), page.total);
        if got == 0 || proposals.len() >= page.total {
            break;
        }
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&saved, serde_json::to_string_pretty(&proposals)?)?;
    let labels = read_labels(&path.join(".context/work-recall-eval/to-label.jsonl"));
    let md = dir.join("classify-memories.md");
    std::fs::write(&md, report(&proposals, &judge, &labels))?;
    println!("Proposed classes for {} memories (judge: {judge}): {}\nNothing was written. Review it, then rerun with --apply.", proposals.len(), md.display());
    Ok(())
}

/// `(node_id, label)` for every labelled row in the revision-1 sample, or
/// none when there's no sample.
fn read_labels(file: &Path) -> Vec<(i64, String)> {
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| Some((v["node_id"].as_i64()?, v["label"].as_str().filter(|l| !l.is_empty())?.to_string())))
        .collect()
}

/// The review page: counts per proposed class, every change, and the
/// labelled sample's rows against their proposed class (an in-sample fit:
/// the title words came from those rows).
fn report(proposals: &[Proposal], judge: &str, labels: &[(i64, String)]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for p in proposals {
        *counts.entry(p.memory_class.as_str()).or_default() += 1;
    }
    let mut out = format!("# Proposed memory classes (local only, git-ignored)\n\nJudge: {judge}. {} memories. Nothing is written until `--apply`.\n\n| class | memories |\n|---|---|\n", proposals.len());
    for (class, n) in &counts {
        out += &format!("| {class} | {n} |\n");
    }

    let by_id: BTreeMap<i64, &Proposal> = proposals.iter().map(|p| (p.node_id, p)).collect();
    if !labels.is_empty() {
        let mut fit: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for (id, label) in labels {
            let class = by_id.get(id).map(|p| p.memory_class.as_str()).unwrap_or("(gone)");
            *fit.entry((label.as_str(), class)).or_default() += 1;
        }
        out += "\n## Labelled sample against proposed class\n\nIn-sample: the status title words were picked from these rows, so this is a fit, not a validation.\n\n| label | class | rows |\n|---|---|---|\n";
        for ((label, class), n) in &fit {
            out += &format!("| {label} | {class} | {n} |\n");
        }
    }

    out += "\n## Changes\n\n| id | kind | from | to | until | title |\n|---|---|---|---|---|---|\n";
    for p in proposals.iter().filter(|p| p.changes()) {
        out += &format!("| {} | {} | {} | {} | {} | {} |\n", p.node_id, p.kind, p.current, p.memory_class, p.class_until.as_deref().unwrap_or(""), p.title.replace('|', "\\|"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: i64, title: &str, current: &str, class: &str) -> Proposal {
        Proposal { node_id: id, kind: "note".into(), title: title.into(), current: current.into(), memory_class: class.into(), class_until: None }
    }

    #[test]
    fn the_report_counts_classes_lists_only_changes_and_fits_the_labels() {
        let proposals = [p(1, "Viewer shipped", "durable", "status"), p(2, "Role | missing", "durable", "durable"), p(3, "Try batches", "durable", "experimental")];
        let labels = [(1, "noise".to_string()), (1, "noise".to_string()), (2, "useful".to_string()), (9, "useful".to_string())];
        let md = report(&proposals, "llm", &labels);
        assert!(md.contains("| status | 1 |") && md.contains("| durable | 1 |") && md.contains("| experimental | 1 |"), "{md}");
        assert!(md.contains("| noise | status | 2 |") && md.contains("| useful | durable | 1 |") && md.contains("| useful | (gone) | 1 |"), "{md}");
        assert!(md.contains("| 1 | note | durable | status |") && !md.contains("| 2 | note |"), "an unchanged memory isn't a change: {md}");
        assert!(proposals[0].changes() && !proposals[1].changes());
    }
}
