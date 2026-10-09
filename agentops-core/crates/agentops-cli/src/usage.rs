//! `agentops usage sync` — parses a local Claude Code JSONL session
//! transcript (`~/.claude/projects/<sanitized-path>/<session-id>.jsonl`,
//! `session-id` a UUID matching the filename) and syncs per-session
//! token/cost totals (Module 8, usage/knowledge-reuse tracking).
//! Deliberately requires `--path` explicitly rather than trying to reverse
//! Claude Code's `/`-to-`-` directory-name sanitization back into a real
//! filesystem path — that reversal is ambiguous whenever the real path
//! itself contains a dash.
//!
//! **Local vs. remote**: a repo connected via `agentops connect --remote`
//! has its real graph store on that remote server, not this machine's
//! `.context/graph.db` — writing usage data locally in that case would
//! land in an orphaned store nothing ever reads (the same class of mistake
//! flagged for gotchas/notes: always check `.context/agentops-remote.json`
//! before a local write). `main.rs`'s `usage_sync_command` is the one that
//! branches on that marker; this module only knows how to parse local
//! JSONL (`collect_usage_entries`), write locally (`write_usage_locally`),
//! or push to a remote server (`push_usage_remote`) — never which one to
//! pick.
//!
//! `UsageEntry::session_id` here is Claude Code's own session UUID (the
//! JSONL filename), *not* necessarily the same string an agent passes as
//! `session_id` to an MCP tool call — see `agentops-api::usage`'s
//! heuristic join for why those two identifiers can't be assumed equal.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use agentops_graph::{GraphStore, NewSessionUsage};
use anyhow::{Context, Result};
use serde::Serialize;

/// One parsed `(session_id, model)` bucket, ready to either upsert locally
/// or serialize into a remote `POST /repos/{id}/usage/sync` body — see
/// this module's doc comment for which caller picks which.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UsageEntry {
    pub session_id: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_estimate_usd: f64,
    /// Largest single-request context in this bucket; 0 if unknown.
    #[serde(default)]
    pub peak_context_tokens: i64,
    pub session_started_at: String,
    pub session_ended_at: String,
}

#[derive(Default)]
struct Aggregate {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    peak_context_tokens: i64,
    // ISO-8601 timestamps compare correctly as plain strings (same format,
    // same 'Z'/offset convention throughout one file) -- no need to parse
    // into a real datetime type just to find min/max.
    started_at: String,
    ended_at: String,
}

/// Claude Code's JSONL transcripts don't carry a `costUSD` field, so a
/// hand-maintained rate table (`agentops_llm::pricing`, shared with the
/// LLM spend ledger) is the only way to produce a cost estimate at all.
/// `session_usage.cost_estimate_usd` is NOT NULL, so a model with no known
/// rate records 0.0 here -- `collect_usage_entries` prints a warning naming
/// it rather than letting the total pass as complete.
fn cost_estimate_usd(model: &str, input_tokens: i64, output_tokens: i64, cache_read_tokens: i64, cache_write_tokens: i64) -> Option<f64> {
    agentops_llm::pricing::cost_estimate_usd("anthropic", model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens)
}

/// `/` -> `-` — Claude Code's own convention for naming a project's session
/// directory under `~/.claude/projects/`. Verified empirically against a
/// real local `~/.claude/projects/` directory (e.g.
/// `/Users/x/Repos/y` -> `-Users-x-Repos-y`), not assumed from docs.
fn sanitize_project_path(canonical_path: &Path) -> String {
    canonical_path.display().to_string().replace('/', "-")
}

fn default_claude_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude")
}

/// One message's usage, as the *last* JSONL line carrying its
/// `message.id` reported it.
struct MessageUsage {
    model: String,
    timestamp: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
}

/// Every API message in one JSONL file, deduped by `message.id`. Claude
/// Code writes one `assistant` line per content block (thinking, text,
/// tool_use) of the same message, each repeating that message's *full*
/// usage -- summing per line overcounted a real session ~2.4x. A line with
/// no `message.id` (older formats) is kept as its own message.
fn read_session_messages(path: &Path) -> Result<Vec<MessageUsage>> {
    let mut by_id: HashMap<String, MessageUsage> = HashMap::new();
    let mut anonymous = Vec::new();
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    for line in std::io::BufReader::new(file).lines() {
        // A session file can be actively appended to by a running
        // Claude Code process — a trailing partial/corrupt line is
        // expected, not an error; skip it and keep going.
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if value.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let message = value.get("message");
        let Some(usage) = message.and_then(|m| m.get("usage")) else { continue };
        let get_u = |key: &str| usage.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
        let parsed = MessageUsage {
            model: message.and_then(|m| m.get("model")).and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            timestamp: value.get("timestamp").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            input_tokens: get_u("input_tokens"),
            output_tokens: get_u("output_tokens"),
            cache_read_tokens: get_u("cache_read_input_tokens"),
            cache_write_tokens: get_u("cache_creation_input_tokens"),
        };
        match message.and_then(|m| m.get("id")).and_then(|v| v.as_str()) {
            Some(id) => {
                by_id.insert(id.to_string(), parsed);
            }
            None => anonymous.push(parsed),
        }
    }
    anonymous.extend(by_id.into_values());
    Ok(anonymous)
}

fn add_messages(totals: &mut HashMap<(String, String), Aggregate>, session_id: &str, messages: Vec<MessageUsage>) {
    for m in messages {
        let agg = totals.entry((session_id.to_string(), m.model)).or_default();
        agg.input_tokens += m.input_tokens;
        agg.output_tokens += m.output_tokens;
        agg.cache_read_tokens += m.cache_read_tokens;
        agg.cache_write_tokens += m.cache_write_tokens;
        agg.peak_context_tokens = agg.peak_context_tokens.max(m.input_tokens + m.cache_read_tokens + m.cache_write_tokens);
        if !m.timestamp.is_empty() {
            if agg.started_at.is_empty() || m.timestamp < agg.started_at {
                agg.started_at = m.timestamp.clone();
            }
            if m.timestamp > agg.ended_at {
                agg.ended_at = m.timestamp;
            }
        }
    }
}

/// One accumulated `(session_id, model)` bucket per session file under a
/// repo's Claude Code project directory, plus a `<session>#subagents`
/// bucket for each session's `<session>/subagents/*.jsonl` transcripts
/// (`agentops_graph::SUBAGENT_SESSION_SUFFIX`) -- a plain `read_dir` of the
/// project dir never sees those, and they're real spend.
fn accumulate_usage(projects_dir: &Path) -> Result<(HashMap<(String, String), Aggregate>, usize)> {
    let mut totals: HashMap<(String, String), Aggregate> = HashMap::new();
    let mut files_scanned = 0;

    let entries = std::fs::read_dir(projects_dir).with_context(|| format!("reading {}", projects_dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let session_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown-session").to_string();
        files_scanned += 1;
        add_messages(&mut totals, &session_id, read_session_messages(&path)?);

        let subagents_dir = projects_dir.join(&session_id).join("subagents");
        if let Ok(sub_entries) = std::fs::read_dir(&subagents_dir) {
            let subagent_bucket = format!("{session_id}{}", agentops_graph::SUBAGENT_SESSION_SUFFIX);
            for sub in sub_entries {
                let sub_path = sub?.path();
                if sub_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                files_scanned += 1;
                add_messages(&mut totals, &subagent_bucket, read_session_messages(&sub_path)?);
            }
        }
    }

    Ok((totals, files_scanned))
}

/// Parses `repo_path`'s Claude Code session files (under `claude_home`, or
/// `~/.claude` if `None`) into one `UsageEntry` per `(session_id, model)`
/// pair found — pure parsing, no store/network access. `repo_path` must be
/// the exact directory Claude Code was run from — see this module's own
/// doc comment for why the sanitized project-directory name isn't reversed
/// instead.
pub fn collect_usage_entries(repo_path: &Path, claude_home: Option<&Path>) -> Result<(Vec<UsageEntry>, usize)> {
    let canonical = repo_path.canonicalize().with_context(|| format!("canonicalizing {}", repo_path.display()))?;
    let claude_home = claude_home.map(PathBuf::from).unwrap_or_else(default_claude_home);
    let projects_dir = claude_home.join("projects").join(sanitize_project_path(&canonical));

    if !projects_dir.exists() {
        return Ok((Vec::new(), 0));
    }

    let (totals, files_scanned) = accumulate_usage(&projects_dir)?;
    let mut out = Vec::with_capacity(totals.len());
    for ((session_id, model), agg) in totals {
        if agg.started_at.is_empty() {
            // No line in this session/model bucket had a timestamp --
            // session_started_at/ended_at are required downstream, and
            // there's nothing safe to write; skip rather than fabricate one.
            continue;
        }
        let has_tokens = agg.input_tokens + agg.output_tokens + agg.cache_read_tokens + agg.cache_write_tokens > 0;
        let cost_estimate_usd = match cost_estimate_usd(&model, agg.input_tokens, agg.output_tokens, agg.cache_read_tokens, agg.cache_write_tokens) {
            Some(cost) => cost,
            None => {
                // Zero-token buckets (e.g. Claude Code's `<synthetic>` model
                // entries) cost nothing either way -- only warn when real
                // usage went unpriced.
                if has_tokens {
                    eprintln!("warning: no known rate for model {model:?} -- its tokens are recorded but priced at $0.00; update agentops_llm::pricing");
                }
                0.0
            }
        };
        out.push(UsageEntry {
            session_id,
            model,
            input_tokens: agg.input_tokens,
            output_tokens: agg.output_tokens,
            cache_read_tokens: agg.cache_read_tokens,
            cache_write_tokens: agg.cache_write_tokens,
            cost_estimate_usd,
            peak_context_tokens: agg.peak_context_tokens,
            session_started_at: agg.started_at,
            session_ended_at: agg.ended_at,
        });
    }

    Ok((out, files_scanned))
}

/// Upserts every entry into `store` directly — the local/self-hosted path,
/// where this machine's own `.context/graph.db` (or `AGENTOPS_DATABASE_URL`)
/// *is* the repo's real graph store.
pub fn write_usage_locally(store: &dyn GraphStore, repo: &str, entries: &[UsageEntry]) -> Result<usize> {
    for entry in entries {
        store.upsert_session_usage(NewSessionUsage {
            repo: repo.to_string(),
            session_id: entry.session_id.clone(),
            model: entry.model.clone(),
            input_tokens: entry.input_tokens,
            output_tokens: entry.output_tokens,
            cache_read_tokens: entry.cache_read_tokens,
            cache_write_tokens: entry.cache_write_tokens,
            cost_estimate_usd: entry.cost_estimate_usd,
            peak_context_tokens: entry.peak_context_tokens,
            session_started_at: entry.session_started_at.clone(),
            session_ended_at: entry.session_ended_at.clone(),
        })?;
    }
    Ok(entries.len())
}

/// Pushes every entry to a hosted deployment's `POST /repos/{id}/usage/sync`
/// — the counterpart to `write_usage_locally` for a repo connected via
/// `agentops connect --remote`, using the same Bearer `api_key`
/// `connect_remote` already obtained (device-login or `--api-key`) and
/// persisted in `.context/agentops-remote.json`.
pub fn push_usage_remote(server_url: &str, connection_id: &str, api_key: &str, entries: &[UsageEntry]) -> Result<usize> {
    let url = format!("{server_url}/repos/{connection_id}/usage/sync");
    let mut response = ureq::post(&url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(serde_json::json!({ "entries": entries }))
        .with_context(|| format!("calling POST {url}"))?;
    if !response.status().is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("POST {url} returned {}: {body}", response.status());
    }
    let body: serde_json::Value = response.body_mut().read_json().with_context(|| format!("parsing POST {url} response"))?;
    Ok(body.get("synced").and_then(|v| v.as_u64()).unwrap_or(entries.len() as u64) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_jsonl(dir: &Path, session_id: &str, lines: &[&str]) {
        std::fs::write(dir.join(format!("{session_id}.jsonl")), lines.join("\n")).unwrap();
    }

    #[test]
    fn sanitize_project_path_replaces_slashes_with_dashes() {
        assert_eq!(sanitize_project_path(Path::new("/Users/x/Repos/y")), "-Users-x-Repos-y");
    }

    #[test]
    fn accumulate_usage_sums_tokens_across_multiple_assistant_turns_in_one_file() {
        let dir = tempfile::tempdir().unwrap();
        write_jsonl(
            dir.path(),
            "sess-1",
            &[
                r#"{"type":"assistant","timestamp":"2026-09-03T00:00:00Z","message":{"model":"claude-sonnet-5","usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":5,"cache_creation_input_tokens":1}}}"#,
                r#"{"type":"assistant","timestamp":"2026-09-03T00:05:00Z","message":{"model":"claude-sonnet-5","usage":{"input_tokens":3,"output_tokens":7,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
                r#"{"type":"user","timestamp":"2026-09-03T00:02:00Z"}"#,
            ],
        );

        let (totals, files_scanned) = accumulate_usage(dir.path()).unwrap();
        assert_eq!(files_scanned, 1);
        let agg = &totals[&("sess-1".to_string(), "claude-sonnet-5".to_string())];
        assert_eq!(agg.input_tokens, 13);
        assert_eq!(agg.output_tokens, 27);
        assert_eq!(agg.cache_read_tokens, 5);
        assert_eq!(agg.cache_write_tokens, 1);
        assert_eq!(agg.started_at, "2026-09-03T00:00:00Z");
        assert_eq!(agg.ended_at, "2026-09-03T00:05:00Z");
    }

    /// Claude Code writes one line per content block, each repeating the
    /// whole message's usage -- real shape, from a 2026-10-09 transcript.
    #[test]
    fn accumulate_usage_counts_a_message_split_across_content_block_lines_once() {
        let dir = tempfile::tempdir().unwrap();
        write_jsonl(
            dir.path(),
            "sess-1",
            &[
                r#"{"type":"assistant","timestamp":"2026-10-09T00:00:00Z","message":{"id":"msg_a","model":"claude-opus-5-5","content":[{"type":"thinking"}],"usage":{"input_tokens":2,"output_tokens":300,"cache_read_input_tokens":20000,"cache_creation_input_tokens":500}}}"#,
                r#"{"type":"assistant","timestamp":"2026-10-09T00:00:01Z","message":{"id":"msg_a","model":"claude-opus-5-5","content":[{"type":"tool_use"}],"usage":{"input_tokens":2,"output_tokens":300,"cache_read_input_tokens":20000,"cache_creation_input_tokens":500}}}"#,
                r#"{"type":"assistant","timestamp":"2026-10-09T00:01:00Z","message":{"id":"msg_b","model":"claude-opus-5-5","content":[{"type":"text"}],"usage":{"input_tokens":1,"output_tokens":50,"cache_read_input_tokens":20500,"cache_creation_input_tokens":100}}}"#,
            ],
        );

        let (totals, _) = accumulate_usage(dir.path()).unwrap();
        let agg = &totals[&("sess-1".to_string(), "claude-opus-5-5".to_string())];
        assert_eq!((agg.input_tokens, agg.output_tokens, agg.cache_read_tokens, agg.cache_write_tokens), (3, 350, 40500, 600));
        assert_eq!(agg.peak_context_tokens, 20601, "largest single request: 1 + 20500 + 100");
    }

    #[test]
    fn accumulate_usage_includes_subagent_transcripts_in_their_own_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let line = |id: &str, out: i64| format!(r#"{{"type":"assistant","timestamp":"2026-10-09T00:00:00Z","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":1,"output_tokens":{out},"cache_read_input_tokens":10,"cache_creation_input_tokens":0}}}}}}"#);
        write_jsonl(dir.path(), "sess-1", &[&line("m1", 100)]);
        let subagents = dir.path().join("sess-1").join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        write_jsonl(&subagents, "agent-a", &[&line("s1", 7)]);
        write_jsonl(&subagents, "agent-b", &[&line("s2", 5)]);

        let (totals, files_scanned) = accumulate_usage(dir.path()).unwrap();
        assert_eq!(files_scanned, 3);
        assert_eq!(totals[&("sess-1".to_string(), "claude-opus-5-5".to_string())].output_tokens, 100);
        let sub = &totals[&(format!("sess-1{}", agentops_graph::SUBAGENT_SESSION_SUFFIX), "claude-opus-5-5".to_string())];
        assert_eq!(sub.output_tokens, 12, "both subagents, aggregated under the parent session");
    }

    #[test]
    fn accumulate_usage_tolerates_a_trailing_corrupt_line() {
        let dir = tempfile::tempdir().unwrap();
        write_jsonl(
            dir.path(),
            "sess-2",
            &[r#"{"type":"assistant","timestamp":"2026-09-03T00:00:00Z","message":{"model":"claude-sonnet-5","usage":{"input_tokens":1,"output_tokens":1}}}"#, r#"{"type":"assistant","message":"#],
        );

        let (totals, _) = accumulate_usage(dir.path()).unwrap();
        assert_eq!(totals.len(), 1, "the corrupt trailing line must be skipped, not fail the whole file");
    }

    #[test]
    fn collect_usage_entries_returns_empty_when_no_claude_project_dir_exists() {
        let repo_dir = tempfile::tempdir().unwrap();
        let claude_home = tempfile::tempdir().unwrap();

        let (entries, files_scanned) = collect_usage_entries(repo_dir.path(), Some(claude_home.path())).unwrap();
        assert_eq!(files_scanned, 0);
        assert!(entries.is_empty());
    }

    #[test]
    fn write_usage_locally_is_idempotent_when_re_run_against_a_growing_session_file() {
        let repo_dir = tempfile::tempdir().unwrap();
        let claude_home = tempfile::tempdir().unwrap();
        let canonical = repo_dir.path().canonicalize().unwrap();
        let projects_dir = claude_home.path().join("projects").join(sanitize_project_path(&canonical));
        std::fs::create_dir_all(&projects_dir).unwrap();
        write_jsonl(
            &projects_dir,
            "sess-3",
            &[r#"{"type":"assistant","timestamp":"2026-09-03T00:00:00Z","message":{"model":"claude-sonnet-5","usage":{"input_tokens":100,"output_tokens":50}}}"#],
        );

        let store = agentops_graph::SqliteGraphStore::open_in_memory().unwrap();
        let repo = "test-repo";
        let (entries1, _) = collect_usage_entries(repo_dir.path(), Some(claude_home.path())).unwrap();
        let synced1 = write_usage_locally(&store, repo, &entries1).unwrap();
        assert_eq!(synced1, 1);

        // The session file grows -- re-syncing must update the same row,
        // not create a second one.
        write_jsonl(
            &projects_dir,
            "sess-3",
            &[
                r#"{"type":"assistant","timestamp":"2026-09-03T00:00:00Z","message":{"model":"claude-sonnet-5","usage":{"input_tokens":100,"output_tokens":50}}}"#,
                r#"{"type":"assistant","timestamp":"2026-09-03T00:10:00Z","message":{"model":"claude-sonnet-5","usage":{"input_tokens":40,"output_tokens":20}}}"#,
            ],
        );
        let (entries2, _) = collect_usage_entries(repo_dir.path(), Some(claude_home.path())).unwrap();
        let synced2 = write_usage_locally(&store, repo, &entries2).unwrap();
        assert_eq!(synced2, 1);

        let rows = store.session_usage_for_repo(repo).unwrap();
        assert_eq!(rows.len(), 1, "re-syncing must update in place, not insert a second row: {rows:?}");
        assert_eq!(rows[0].input_tokens, 140);
        assert_eq!(rows[0].output_tokens, 70);
    }
}
