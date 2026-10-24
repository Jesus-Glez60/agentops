//! `agentops consolidate` (milestone 4 item 4, ADR 0056): consolidation
//! proposals. `--session <id>` is what the `SessionEnd` hook starts,
//! detached: for a remote repo the server runs it with the tenant's key;
//! locally it uses `AGENTOPS_ANTHROPIC_API_KEY`. With no key nothing runs.
//! `--all --dry-run` lists the repo's merge candidates without writing,
//! into `.context/consolidate.dry-run.{json,md}` (titles stay in that
//! local file); `--generate <id>...` proposes exactly the picked ones.
//! Nothing here accepts a proposal: a person does, in the Gotchas
//! workspace.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::recall_eval::remote_send_within;
use crate::remote_marker::read_remote_marker;

/// A run makes a few LLM calls under a 20,000-token cap.
const TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) fn run(path: &Path, session: Option<&str>, all: bool, dry_run: bool, generate: &[String]) -> Result<()> {
    let marker = read_remote_marker(path);
    let reply = if let Some(session) = session {
        match &marker {
            Some(m) => remote_send_within(m, "memories/consolidate", Some(&json!({ "session_id": session })), TIMEOUT)?,
            None => {
                let (store, repo) = (agentops_mcp::open_store(path)?, agentops_mcp::repo_name(path));
                agentops_mcp::proposals::run_for_session(path, None, store.as_ref(), &repo, session, agentops_llm::LlmConfig::from_env().ok().as_ref())?
            }
        }
    } else if all && dry_run {
        let listed = match &marker {
            Some(m) => remote_send_within(m, "memories/consolidate/candidates", Some(&json!({})), TIMEOUT)?,
            None => {
                let (store, repo) = (agentops_mcp::open_store(path)?, agentops_mcp::repo_name(path));
                let mut out = Vec::new();
                for c in agentops_mcp::proposals::repo_candidates(store.as_ref(), &repo)? {
                    let sources: Vec<Value> = c.sources.iter().map(|&id| -> Result<Value> { let n = store.get_node(&repo, id)?; Ok(json!({ "id": id, "kind": n.as_ref().map(|n| n.kind.as_db_str()), "title": n.and_then(|n| n.name) })) }).collect::<Result<_>>()?;
                    out.push(json!({ "id": c.id, "purpose": c.purpose, "cosine": c.cosine, "sources": sources }));
                }
                json!({ "candidates": out })
            }
        };
        save_dry_run(path, &listed)?;
        return Ok(());
    } else if !generate.is_empty() {
        match &marker {
            Some(m) => remote_send_within(m, "memories/consolidate", Some(&json!({ "candidates": generate })), TIMEOUT)?,
            None => {
                let (store, repo) = (agentops_mcp::open_store(path)?, agentops_mcp::repo_name(path));
                let picked = generate.iter().map(|c| agentops_mcp::proposals::Candidate::from_id(c).with_context(|| format!("not a candidate id: {c}"))).collect::<Result<Vec<_>>>()?;
                let picked = agentops_mcp::proposals::unblocked(store.as_ref(), &repo, picked)?;
                match agentops_llm::LlmConfig::from_env() {
                    Ok(config) => json!({ "ran": true, "log": agentops_mcp::proposals::generate(path, None, store.as_ref(), &repo, &config, &picked, None, "consolidate-batch")? }),
                    Err(_) => json!({ "ran": false, "reason": "no LLM key (AGENTOPS_ANTHROPIC_API_KEY)", "candidates": picked.len() }),
                }
            }
        }
    } else {
        anyhow::bail!("give --session <id>, --all --dry-run, or --generate <candidate id>...");
    };
    println!("{}", serde_json::to_string(&reply)?);
    Ok(())
}

/// Saves the batch dry run for review: the JSON (stable candidate ids,
/// their sources and cosines) and a Markdown table.
fn save_dry_run(path: &Path, listed: &Value) -> Result<()> {
    let candidates = listed["candidates"].as_array().cloned().unwrap_or_default();
    let dir = path.join(".context");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("consolidate.dry-run.json"), serde_json::to_string_pretty(&candidates)?)?;
    let mut md = format!("# Consolidation dry run\n\n{} merge candidates (cosine {} to {}). Nothing was written. Generate the ones worth it with `agentops consolidate --generate <id>...`.\n\n| Candidate | Cosine | Sources |\n| --- | --- | --- |\n", candidates.len(), agentops_mcp::proposals::MERGE_MIN_SCORE, agentops_graph::similarity::DUPLICATE_MIN_SCORE);
    for c in &candidates {
        let sources = c["sources"].as_array().map(|s| s.iter().map(|s| format!("{} [{}] {}", s["id"], s["kind"].as_str().unwrap_or("?"), s["title"].as_str().unwrap_or("(gone)").replace('|', "\\|"))).collect::<Vec<_>>().join("<br>")).unwrap_or_default();
        md += &format!("| {} | {} | {sources} |\n", c["id"].as_str().unwrap_or(""), c["cosine"].as_f64().map(|v| format!("{v:.3}")).unwrap_or_default());
    }
    std::fs::write(dir.join("consolidate.dry-run.md"), md)?;
    println!("{} merge candidates; review .context/consolidate.dry-run.md.", candidates.len());
    Ok(())
}
