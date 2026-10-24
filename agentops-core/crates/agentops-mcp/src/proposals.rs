//! Consolidation proposals (milestone 4 item 4, ADR 0056): at session end,
//! an LLM proposes durable memories from what the session wrote. It merges
//! near-duplicates into one memory, and it distills the lesson out of
//! progress memories. A proposal is a needs-review memory tagged
//! `proposed-by:consolidation`, with its sources recorded and **not
//! applied**. Recall never shows it (`NeedsReview`). Only a person accepts
//! it; accepting supersedes its sources. A rejection is kept with a digest
//! of the sources' text, so the same unchanged sources aren't proposed
//! again. With no LLM key, nothing runs.

use std::collections::HashSet;
use std::path::Path;

use agentops_graph::similarity::{similar_memories, DUPLICATE_MIN_SCORE, MEMORY_KINDS};
use agentops_graph::{GraphStore, MemoryClass, Node, NodeProminence};
use agentops_llm::{ConsolidationPurpose, LlmConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Lowest cosine at which two memories are merge candidates: the band where
/// milestone 3a found same-fact pairs (0.85-0.92). From
/// `DUPLICATE_MIN_SCORE` up, the importer already treats a pair as one.
pub const MERGE_MIN_SCORE: f32 = 0.85;
/// Proposals one session's end may write: few enough to review.
pub const MAX_PROPOSALS_PER_SESSION: usize = 3;
/// Input plus output tokens one run may spend; checked before each call.
pub const RUN_TOKEN_CAP: u64 = 20_000;
/// The tag every proposal carries.
pub const PROPOSED_TAG: &str = "proposed-by:consolidation";
/// `event_kind` logging each run, its description the run's JSON.
pub const CONSOLIDATION_RUN: &str = "consolidation_run";
/// Nearest neighbours looked at per memory for merge pairs.
const MERGE_NEIGHBOURS: usize = 5;
/// Most progress memories one distill call reads, the newest.
const DISTILL_MAX_SOURCES: usize = 6;

/// Something a run may propose from. `id` is stable for the same sources:
/// `m<a>-<b>` for a merge pair, `d<id>-<id>...` for a distill set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    /// `merge` or `distill`.
    pub purpose: String,
    pub sources: Vec<i64>,
    /// A merge pair's cosine.
    #[serde(default)]
    pub cosine: Option<f32>,
}

impl Candidate {
    fn merge(a: i64, b: i64, cosine: f32) -> Self {
        let (a, b) = (a.min(b), a.max(b));
        Candidate { id: format!("m{a}-{b}"), purpose: "merge".into(), sources: vec![a, b], cosine: Some(cosine) }
    }

    fn distill(mut ids: Vec<i64>) -> Self {
        ids.sort();
        Candidate { id: format!("d{}", ids.iter().map(i64::to_string).collect::<Vec<_>>().join("-")), purpose: "distill".into(), sources: ids, cosine: None }
    }
}

impl Candidate {
    /// The candidate an id names (`m<a>-<b>`, `d<id>-...`), as
    /// `consolidate --generate` passes them back from the dry run.
    pub fn from_id(id: &str) -> Option<Candidate> {
        let ids = |rest: &str| rest.split('-').map(str::parse::<i64>).collect::<std::result::Result<Vec<_>, _>>().ok().filter(|v| !v.is_empty());
        if let Some(rest) = id.strip_prefix('m') {
            let v = ids(rest)?;
            return (v.len() == 2).then(|| Candidate { cosine: None, ..Candidate::merge(v[0], v[1], 0.0) });
        }
        id.strip_prefix('d').and_then(ids).map(Candidate::distill)
    }
}

/// What one run did, logged as a `consolidation_run` event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunLog {
    pub candidates: usize,
    /// The proposal node ids written.
    pub proposals: Vec<i64>,
    /// Candidates the model judged not worth a memory.
    pub not_worth: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Estimated, from `agentops_llm::pricing`; `None` for an unpriced model.
    pub usd: Option<f64>,
    pub cap_reached: bool,
    /// The LLM call that stopped the run early (a rate limit, a provider
    /// error); what was written before it stays.
    #[serde(default)]
    pub error: Option<String>,
    pub provider: String,
    pub model: String,
}

/// A memory a merge may draw on: recallable now, not a proposal itself.
fn mergeable(node: &Node, today: &str) -> bool {
    MEMORY_KINDS.contains(&node.kind) && node.prominence.recallable() && node.memory_class != MemoryClass::Progress && node.memory_class.recallable_at(today, node.class_until.as_deref())
}

/// The near-duplicate pairs among `ids` and their neighbours: cosine in
/// [`MERGE_MIN_SCORE`, `DUPLICATE_MIN_SCORE`), both mergeable.
fn merge_pairs(store: &dyn GraphStore, repo: &str, ids: &[i64]) -> Result<Vec<Candidate>> {
    let today = agentops_graph::utc_today();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &id in ids {
        let Some(node) = store.get_node(repo, id)?.filter(|n| mergeable(n, &today)) else { continue };
        let Some(embedding) = store.get_embedding(repo, node.id)? else { continue };
        for (other, cosine) in similar_memories(store, repo, &embedding, Some(node.id), MERGE_NEIGHBOURS)? {
            if (MERGE_MIN_SCORE..DUPLICATE_MIN_SCORE).contains(&cosine) && mergeable(&other, &today) {
                let c = Candidate::merge(node.id, other.id, cosine);
                if seen.insert(c.id.clone()) {
                    out.push(c);
                }
            }
        }
    }
    out.sort_by(|a, b| b.cosine.unwrap_or(0.0).total_cmp(&a.cosine.unwrap_or(0.0)).then(a.id.cmp(&b.id)));
    Ok(out)
}

/// A session's candidates: merges for the memories it wrote, and one
/// distill set of the progress memories it wrote or followed. Empty when it
/// wrote none and touched no progress memory.
pub fn session_candidates(store: &dyn GraphStore, repo: &str, session_id: &str) -> Result<Vec<Candidate>> {
    let events = store.session_events(repo, session_id)?;
    let written: Vec<i64> = {
        let mut ids: Vec<i64> = crate::work_recall::written_ids(&events).into_iter().collect();
        ids.sort();
        ids
    };
    let followed = events.iter().filter(|e| e.event_kind == crate::hooks::MEMORY_FOLLOWED).filter_map(|e| e.node_id);
    let mut progress = Vec::new();
    for id in written.iter().copied().chain(followed) {
        if !progress.contains(&id) && store.get_node(repo, id)?.is_some_and(|n| MEMORY_KINDS.contains(&n.kind) && n.memory_class == MemoryClass::Progress) {
            progress.push(id);
        }
    }
    let mut out = merge_pairs(store, repo, &written)?;
    if !progress.is_empty() {
        progress.sort_by(|a, b| b.cmp(a));
        progress.truncate(DISTILL_MAX_SOURCES);
        out.push(Candidate::distill(progress));
    }
    unblocked(store, repo, out)
}

/// Every merge pair in the repo, for `consolidate --all --dry-run`.
pub fn repo_candidates(store: &dyn GraphStore, repo: &str) -> Result<Vec<Candidate>> {
    let mut ids = Vec::new();
    for &kind in MEMORY_KINDS {
        ids.extend(store.nodes_by_kind(repo, kind)?.into_iter().map(|n| n.id));
    }
    ids.sort();
    let pairs = merge_pairs(store, repo, &ids)?;
    unblocked(store, repo, pairs)
}

/// `candidates` without those already waiting as a proposal, or rejected
/// while their sources read exactly as they do now.
pub fn unblocked(store: &dyn GraphStore, repo: &str, candidates: Vec<Candidate>) -> Result<Vec<Candidate>> {
    let proposals = store.proposals(repo)?;
    let mut out = Vec::new();
    for c in candidates {
        let same: Vec<_> = proposals.iter().filter(|p| sorted(&p.source_ids) == sorted(&c.sources)).collect();
        if same.iter().any(|p| p.state == "proposed") {
            continue;
        }
        if !same.is_empty() {
            let digest = sources_digest(store, repo, &c.sources)?;
            if same.iter().any(|p| p.state == "rejected" && p.sources_digest == digest) {
                continue;
            }
        }
        out.push(c);
    }
    Ok(out)
}

fn sorted(ids: &[i64]) -> Vec<i64> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids
}

/// SHA-256 of the sorted source ids with each one's title and body: equal
/// exactly when the sources read the same.
pub fn sources_digest(store: &dyn GraphStore, repo: &str, ids: &[i64]) -> Result<String> {
    let mut hasher = Sha256::new();
    for id in sorted(ids) {
        let node = store.get_node(repo, id)?;
        hasher.update(format!("{id}\u{1f}{}\u{1f}{}\u{1e}", node.as_ref().and_then(|n| n.name.as_deref()).unwrap_or(""), node.as_ref().and_then(|n| n.content.as_deref()).unwrap_or("")));
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Proposes from `candidates` in order, until `max` proposals (when given)
/// or `RUN_TOKEN_CAP` tokens; each proposal is written as a needs-review
/// memory with its sources recorded. Logs the run under `session_id`.
pub fn generate(repo_path: &Path, notes_dir: Option<&Path>, store: &dyn GraphStore, repo: &str, config: &LlmConfig, candidates: &[Candidate], max: Option<usize>, session_id: &str) -> Result<RunLog> {
    let mut log = RunLog { candidates: candidates.len(), provider: config.provider.name().to_string(), model: config.model.clone(), ..Default::default() };
    let mut usd = 0.0;
    let mut priced = true;
    for c in candidates {
        if max.is_some_and(|m| log.proposals.len() >= m) {
            break;
        }
        if log.input_tokens + log.output_tokens >= RUN_TOKEN_CAP {
            log.cap_reached = true;
            break;
        }
        let mut sources = Vec::new();
        for &id in &c.sources {
            if let Some(n) = store.get_node(repo, id)? {
                sources.push((n.kind, n.name.unwrap_or_default(), n.content.unwrap_or_default()));
            }
        }
        if sources.len() != c.sources.len() {
            continue; // a source is gone
        }
        let purpose = if c.purpose == "merge" { ConsolidationPurpose::Merge } else { ConsolidationPurpose::Distill };
        let (proposed, call) = match agentops_llm::propose_memory(config, purpose, &sources) {
            Ok(done) => done,
            Err(e) => {
                log.error = Some(format!("{e:#}"));
                break;
            }
        };
        log.input_tokens += call.input_tokens;
        log.output_tokens += call.output_tokens;
        match agentops_llm::pricing::cost_estimate_usd(&call.provider, &call.model, call.input_tokens as i64, call.output_tokens as i64, 0, 0) {
            Some(cost) => usd += cost,
            None => priced = false,
        }
        let Some(proposed) = proposed else {
            log.not_worth += 1;
            continue;
        };
        log.proposals.push(write_proposal(repo_path, notes_dir, store, repo, &proposed, &c.sources)?);
    }
    log.usd = priced.then_some(usd);
    store.record_session_event(repo, session_id, "consolidate", &serde_json::to_string(&log)?, None, CONSOLIDATION_RUN)?;
    Ok(log)
}

/// Writes one proposal: a note at `NeedsReview`, durable, tagged, with a
/// curation reason naming its sources, and its `consolidation_proposals` row.
fn write_proposal(repo_path: &Path, notes_dir: Option<&Path>, store: &dyn GraphStore, repo: &str, proposed: &agentops_llm::ProposedMemory, sources: &[i64]) -> Result<i64> {
    let note_type = match proposed.kind.as_str() {
        "gotcha" => agentops_notes::NoteType::Gotcha,
        "decision" => agentops_notes::NoteType::Decision,
        _ => agentops_notes::NoteType::Knowledge,
    };
    // `Write`, not `WriteUnlessDuplicate`: a merge is close to its own
    // sources by design.
    let written = crate::notes::add_note_with(repo_path, &proposed.title, &proposed.body, Some(note_type), &[PROPOSED_TAG.to_string()], notes_dir, true, crate::notes::AddMode::Write)?;
    let id = written.node_id.context("the proposal was written but has no node id")?;
    let ids = sources.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
    store.set_curation(repo, id, NodeProminence::NeedsReview, Some(&format!("proposed by consolidation from {ids}")))?;
    store.set_memory_class(repo, id, MemoryClass::Durable, None)?;
    store.record_proposal(repo, id, &sorted(sources), &sources_digest(store, repo, sources)?)?;
    Ok(id)
}

/// The session-end run: nothing when the session wrote no memory and
/// touched no progress memory, or when there is no LLM key; else at most
/// `MAX_PROPOSALS_PER_SESSION` proposals. Never an error for either.
pub fn run_for_session(repo_path: &Path, notes_dir: Option<&Path>, store: &dyn GraphStore, repo: &str, session_id: &str, config: Option<&LlmConfig>) -> Result<serde_json::Value> {
    let candidates = session_candidates(store, repo, session_id)?;
    if candidates.is_empty() {
        return Ok(serde_json::json!({ "ran": false, "reason": "the session wrote no memory and touched no progress memory" }));
    }
    let Some(config) = config else {
        return Ok(serde_json::json!({ "ran": false, "reason": "no LLM key (Anthropic or Groq)", "candidates": candidates.len() }));
    };
    let log = generate(repo_path, notes_dir, store, repo, config, &candidates, Some(MAX_PROPOSALS_PER_SESSION), session_id)?;
    Ok(serde_json::json!({ "ran": true, "log": log }))
}

/// A waiting proposal, or an error naming why it can't be decided.
fn waiting(store: &dyn GraphStore, repo: &str, proposal_id: i64) -> Result<agentops_graph::ConsolidationProposal> {
    let row = store.proposals(repo)?.into_iter().find(|p| p.proposal_id == proposal_id).with_context(|| format!("{proposal_id} isn't a consolidation proposal"))?;
    anyhow::ensure!(row.state == "proposed", "proposal {proposal_id} was already {}", row.state);
    Ok(row)
}

/// Accepts a proposal (a person's decision, never automatic): it becomes a
/// full, durable memory, and its sources are superseded (temporal, ending
/// today, as `add_note`'s `supersedes`), so recall stops showing them.
pub fn accept(repo_path: &Path, store: &dyn GraphStore, repo: &str, proposal_id: i64) -> Result<()> {
    let row = waiting(store, repo, proposal_id)?;
    let node = store.get_node(repo, proposal_id)?.with_context(|| format!("proposal {proposal_id}'s memory is gone"))?;
    store.set_curation(repo, proposal_id, NodeProminence::Full, None)?;
    crate::notes::apply_memory_class(repo_path, proposal_id, node.name.as_deref().unwrap_or(""), Some((MemoryClass::Durable, None)), &row.source_ids)?;
    store.set_proposal_state(repo, proposal_id, "accepted")
}

/// Rejects a proposal: its memory and note file are deleted, and the
/// rejection is kept with its sources' digest.
pub fn reject(repo_path: &Path, notes_dir: Option<&Path>, store: &dyn GraphStore, repo: &str, proposal_id: i64) -> Result<()> {
    waiting(store, repo, proposal_id)?;
    if let Some(node) = store.get_node(repo, proposal_id)? {
        // The file goes too, or the next ingest would bring the node back.
        if let Some(rel) = node.path.as_deref().and_then(|p| p.strip_prefix("vault:")) {
            let file = agentops_notes::resolve_notes_path(repo_path, notes_dir).join(rel);
            if file.exists() {
                std::fs::remove_file(&file).with_context(|| format!("removing {}", file.display()))?;
            }
        }
        store.delete_nodes(repo, &[proposal_id])?;
    }
    store.set_proposal_state(repo, proposal_id, "rejected")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// An Anthropic stand-in answering one request per reply, in order,
    /// each with `usage` tokens; returns its base URL.
    fn mock_llm(replies: Vec<(&'static str, u64, u64)>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for (text, input, output) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = vec![0u8; 1 << 20];
                let mut got = String::new();
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    got.push_str(&String::from_utf8_lossy(&buf[..n]));
                    let Some((head, body)) = got.split_once("\r\n\r\n") else { continue };
                    let len = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    if n == 0 || body.len() >= len {
                        break;
                    }
                }
                let body = serde_json::json!({ "content": [{ "type": "text", "text": text }], "usage": { "input_tokens": input, "output_tokens": output } }).to_string();
                write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        url
    }

    fn config(url: &str) -> LlmConfig {
        LlmConfig { api_key: "k".into(), model: "claude-sonnet-5".into(), api_url: format!("{url}/v1/messages"), ..Default::default() }
    }

    /// A 384-dim unit vector at `cosine` to the first axis.
    fn at(cosine: f32) -> Vec<f32> {
        let mut v = vec![0.0; agentops_embeddings::EMBEDDING_DIM];
        (v[0], v[1]) = (cosine, (1.0 - cosine * cosine).sqrt());
        v
    }

    struct Repo {
        dir: tempfile::TempDir,
        store: Box<dyn GraphStore>,
        repo: String,
    }

    impl Repo {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = crate::store::open_store(dir.path()).unwrap();
            let repo = crate::scan::repo_name(dir.path());
            Repo { dir, store, repo }
        }
        fn memory(&self, title: &str, body: &str, vector: Vec<f32>) -> i64 {
            let id = crate::notes::add_note_with(self.dir.path(), title, body, Some(agentops_notes::NoteType::Gotcha), &[], None, false, crate::notes::AddMode::Write).unwrap().node_id.unwrap();
            self.store.set_embedding(&self.repo, id, &vector).unwrap();
            id
        }
        fn wrote(&self, session: &str, ids: &[i64]) {
            let ids = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            self.store.record_session_event(&self.repo, session, "mcp__agentops__add_note", &format!("ids {ids}"), None, crate::hooks::MEMORY_WRITTEN).unwrap();
        }
        fn node(&self, id: i64) -> Option<Node> {
            self.store.get_node(&self.repo, id).unwrap()
        }
    }

    const MERGED: &str = r#"{"worth": true, "kind": "gotcha", "title": "The pool runs out under load", "body": "Both say the same."}"#;
    const NOT_WORTH: &str = r#"{"worth": false}"#;

    #[test]
    fn a_session_end_proposes_from_what_it_wrote_and_applies_nothing() {
        let r = Repo::new();
        let a = r.memory("Pool exhaustion under load", "The connection pool runs out at 50 clients.", at(1.0));
        let b = r.memory("Connection pool runs out", "At 50 clients the pool is exhausted.", at(0.88));
        let far = r.memory("Unrelated", "Nothing alike.", at(0.2));
        let progress = r.memory("Pool fix shipped", "Shipped the pool fix.", at(0.1));
        r.store.set_memory_class(&r.repo, progress, MemoryClass::Progress, None).unwrap();
        r.wrote("s", &[b, progress]);

        let candidates = session_candidates(r.store.as_ref(), &r.repo, "s").unwrap();
        assert_eq!(candidates.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec![format!("m{a}-{b}"), format!("d{progress}")], "the near-duplicate pair and the progress set; {far} is too far");

        let url = mock_llm(vec![(MERGED, 900, 100), (NOT_WORTH, 500, 10)]);
        let log = generate(r.dir.path(), None, r.store.as_ref(), &r.repo, &config(&url), &candidates, Some(MAX_PROPOSALS_PER_SESSION), "s").unwrap();
        assert_eq!((log.proposals.len(), log.not_worth, log.input_tokens, log.output_tokens, log.cap_reached), (1, 1, 1400, 110, false));
        let proposal = r.node(log.proposals[0]).unwrap();
        assert_eq!((proposal.prominence, proposal.memory_class), (NodeProminence::NeedsReview, MemoryClass::Durable));
        assert_eq!(proposal.curation_reason.as_deref(), Some(format!("proposed by consolidation from {a}, {b}").as_str()));
        assert!(proposal.content.unwrap_or_default().contains(PROPOSED_TAG));
        for source in [a, b] {
            let n = r.node(source).unwrap();
            assert_eq!((n.prominence, n.memory_class), (NodeProminence::Full, MemoryClass::Durable), "sources are recorded, not applied");
        }
        assert_eq!(r.store.proposals(&r.repo).unwrap()[0].source_ids, vec![a, b]);
        let runs: Vec<_> = r.store.session_events(&r.repo, "s").unwrap().into_iter().filter(|e| e.event_kind == CONSOLIDATION_RUN).collect();
        assert_eq!(serde_json::from_str::<RunLog>(&runs[0].description).unwrap(), log);
        assert!(log.usd.is_some(), "priced");

        let again = session_candidates(r.store.as_ref(), &r.repo, "s").unwrap();
        assert_eq!(again.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec![format!("d{progress}")], "a waiting proposal's sources aren't proposed twice");
    }

    #[test]
    fn a_candidate_id_names_its_sources() {
        assert_eq!(Candidate::from_id("m4-9").map(|c| (c.purpose, c.sources)), Some(("merge".to_string(), vec![4, 9])));
        assert_eq!(Candidate::from_id("d7-3").map(|c| (c.id, c.sources)), Some(("d3-7".to_string(), vec![3, 7])));
        assert_eq!(Candidate::from_id("m4"), None);
        assert_eq!(Candidate::from_id("x1-2"), None);
        assert_eq!(Candidate::from_id("dfoo"), None);
    }

    #[test]
    fn the_token_cap_stops_a_run_and_keeps_what_it_wrote() {
        let r = Repo::new();
        let ids: Vec<i64> = (0..3).map(|i| r.memory(&format!("Twin {i}"), &format!("Twin fact {i}."), at(1.0))).collect();
        let pairs: Vec<Candidate> = vec![Candidate::merge(ids[0], ids[1], 0.9), Candidate::merge(ids[1], ids[2], 0.9)];
        let url = mock_llm(vec![(MERGED, 15_000, 5_000)]);
        let log = generate(r.dir.path(), None, r.store.as_ref(), &r.repo, &config(&url), &pairs, None, "batch").unwrap();
        assert_eq!((log.proposals.len(), log.cap_reached), (1, true));
    }

    #[test]
    fn a_failed_call_stops_the_run_keeps_what_it_wrote_and_is_logged() {
        let r = Repo::new();
        let ids: Vec<i64> = (0..3).map(|i| r.memory(&format!("Twin {i}"), &format!("Twin fact {i}."), at(1.0))).collect();
        let pairs = vec![Candidate::merge(ids[0], ids[1], 0.9), Candidate::merge(ids[1], ids[2], 0.9)];
        // One good reply, then the stand-in is gone: the second call fails.
        let url = mock_llm(vec![(MERGED, 10, 10)]);
        let log = generate(r.dir.path(), None, r.store.as_ref(), &r.repo, &config(&url), &pairs, None, "batch").unwrap();
        assert_eq!(log.proposals.len(), 1);
        assert!(log.error.is_some(), "{log:?}");
        let runs: Vec<_> = r.store.session_events(&r.repo, "batch").unwrap().into_iter().filter(|e| e.event_kind == CONSOLIDATION_RUN).collect();
        assert_eq!(runs.len(), 1, "the failed run is still logged with its cost");
    }

    #[test]
    fn accepting_supersedes_the_sources_and_rejecting_deletes_and_blocks_until_they_change() {
        let r = Repo::new();
        let a = r.memory("Pool exhaustion under load", "The connection pool runs out at 50 clients.", at(1.0));
        let b = r.memory("Connection pool runs out", "At 50 clients the pool is exhausted.", at(0.88));
        let url = mock_llm(vec![(MERGED, 10, 10), (MERGED, 10, 10), (MERGED, 10, 10)]);
        let pair = vec![Candidate::merge(a, b, 0.88)];

        let first = generate(r.dir.path(), None, r.store.as_ref(), &r.repo, &config(&url), &pair, None, "s1").unwrap().proposals[0];
        let file = agentops_notes::resolve_notes_path(r.dir.path(), None).join(r.node(first).unwrap().path.unwrap().strip_prefix("vault:").unwrap());
        assert!(file.exists());
        reject(r.dir.path(), None, r.store.as_ref(), &r.repo, first).unwrap();
        assert!(r.node(first).is_none() && !file.exists(), "the memory and its note file are gone");
        assert!(reject(r.dir.path(), None, r.store.as_ref(), &r.repo, first).is_err(), "decided once");
        assert!(unblocked(r.store.as_ref(), &r.repo, pair.clone()).unwrap().is_empty(), "rejected, sources unchanged");
        r.store.update_node(&r.repo, b, None, None, Some("At 50 clients the pool is exhausted; raise max_size.".into())).unwrap();
        assert_eq!(unblocked(r.store.as_ref(), &r.repo, pair.clone()).unwrap(), pair, "a changed source can be proposed again");

        let second = generate(r.dir.path(), None, r.store.as_ref(), &r.repo, &config(&url), &pair, None, "s2").unwrap().proposals[0];
        accept(r.dir.path(), r.store.as_ref(), &r.repo, second).unwrap();
        let accepted = r.node(second).unwrap();
        assert_eq!((accepted.prominence, accepted.memory_class), (NodeProminence::Full, MemoryClass::Durable));
        for source in [a, b] {
            let n = r.node(source).unwrap();
            assert_eq!((n.memory_class, n.class_until), (MemoryClass::Temporal, Some(agentops_graph::utc_today())), "superseded: recall drops it today");
        }
        assert_eq!(r.store.proposals(&r.repo).unwrap().iter().map(|p| p.state.as_str()).collect::<Vec<_>>(), vec!["rejected", "accepted"]);
        assert!(accept(r.dir.path(), r.store.as_ref(), &r.repo, a).is_err(), "only a proposal can be accepted");
    }

    #[test]
    fn with_no_key_or_nothing_written_a_session_end_does_nothing_and_never_fails() {
        let r = Repo::new();
        assert_eq!(run_for_session(r.dir.path(), None, r.store.as_ref(), &r.repo, "quiet", None).unwrap()["ran"], false);
        let a = r.memory("Pool exhaustion under load", "x", at(1.0));
        let b = r.memory("Connection pool runs out", "y", at(0.88));
        r.wrote("s", &[b]);
        let out = run_for_session(r.dir.path(), None, r.store.as_ref(), &r.repo, "s", None).unwrap();
        assert_eq!((out["ran"].as_bool(), out["candidates"].as_u64()), (Some(false), Some(1)), "{out}");
        assert!(r.store.proposals(&r.repo).unwrap().is_empty() && r.node(a).unwrap().prominence == NodeProminence::Full);
        assert!(r.store.session_events(&r.repo, "s").unwrap().iter().all(|e| e.event_kind != CONSOLIDATION_RUN), "no run, no log");
    }
}
