//! `agentops classify-memories --scope [--host NAME]... [--apply]`
//! (milestone 4 item 3): proposes which memories are tied to one machine,
//! from the host names found in their titles and bodies. The dry run reads
//! every memory from the server and writes nothing there; it saves its
//! proposals to `.context/classify-memories.scope.{json,md}` for the author
//! to review. `--apply` then sends exactly that file to `memories/scope`.
//!
//! The candidate names come from this machine only: its hostname, the
//! `Host` aliases in `~/.ssh/config`, and `--host`. They are matched here
//! and never sent; only the reviewed `applies_to` lists are.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::recall_eval::remote_send_within;
use crate::remote_marker::read_remote_marker;

const TIMEOUT: Duration = Duration::from_secs(60);
/// Memories listed per `memories/classify` page (its maximum).
const PAGE: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ScopeProposal {
    pub(crate) node_id: i64,
    pub(crate) kind: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) current: Option<Vec<String>>,
    /// The proposed hosts: every candidate name the memory mentions.
    pub(crate) applies_to: Vec<String>,
    /// What it would mean for error cues, as seen from this machine.
    pub(crate) effect: String,
}

pub(crate) fn run(path: &Path, apply: bool, extra_hosts: &[String]) -> Result<()> {
    let marker = read_remote_marker(path).with_context(|| format!("classify-memories runs against the server; {} has no .context/agentops-remote.json", path.display()))?;
    let saved = path.join(".context").join("classify-memories.scope.json");
    if apply {
        let proposals: Vec<ScopeProposal> = serde_json::from_str(&std::fs::read_to_string(&saved).with_context(|| format!("run the dry run first; no {}", saved.display()))?)?;
        let assign: Vec<Value> = proposals.iter().map(|p| json!({ "node_id": p.node_id, "applies_to": p.applies_to })).collect();
        let reply = remote_send_within(&marker, "memories/scope", Some(&json!({ "assign": assign })), TIMEOUT)?;
        println!("Scoped {} of {} reviewed memories from {}.", reply["assigned"].as_u64().unwrap_or(0), proposals.len(), saved.display());
        return Ok(());
    }

    let (this_host, names) = local_candidate_names(extra_hosts);
    anyhow::ensure!(!names.is_empty(), "no candidate host names: no hostname, no ~/.ssh/config aliases and no --host");

    let mut proposals = Vec::new();
    let mut offset = 0;
    loop {
        let page = remote_send_within(&marker, "memories/classify", Some(&json!({ "offset": offset, "limit": PAGE, "llm": false })), TIMEOUT)?;
        let listed = page["proposals"].as_array().cloned().unwrap_or_default();
        for m in &listed {
            let Some(id) = m["node_id"].as_i64() else { continue };
            let node = remote_send_within(&marker, &format!("nodes/{id}"), None, TIMEOUT)?;
            let title = node["name"].as_str().unwrap_or("").to_string();
            let text = format!("{title}\n{}", node["content"].as_str().unwrap_or(""));
            let found = mentioned(&text, &names);
            if found.is_empty() {
                continue;
            }
            let current = serde_json::from_value(node["applies_to"].clone()).unwrap_or(None);
            proposals.push(ScopeProposal { node_id: id, kind: m["kind"].as_str().unwrap_or("").to_string(), title, current, effect: effect(&found, this_host.as_deref()), applies_to: found });
        }
        offset += listed.len();
        if listed.is_empty() || offset >= page["total"].as_u64().unwrap_or(0) as usize {
            break;
        }
    }

    std::fs::create_dir_all(path.join(".context"))?;
    std::fs::write(&saved, serde_json::to_string_pretty(&proposals)?)?;
    let md = saved.with_extension("md");
    std::fs::write(&md, render(&proposals, &names, this_host.as_deref()))?;
    println!("{} of {offset} memories mention one of {} host names. Review {} (edit {} to change a proposal), then run with --apply.", proposals.len(), names.len(), md.display(), saved.display());
    Ok(())
}

/// This machine's normalized hostname, and `candidate_names` from it and
/// the user's `~/.ssh/config`.
pub(crate) fn local_candidate_names(extra_hosts: &[String]) -> (Option<String>, Vec<String>) {
    let this_host = crate::hook_capture::hostname().as_deref().and_then(agentops_mcp::hooks::normalize_host);
    let ssh_config = dirs::home_dir().map(|h| h.join(".ssh").join("config")).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let names = candidate_names(this_host.as_deref(), &ssh_config, extra_hosts);
    (this_host, names)
}

/// The names to look for, normalized and deduplicated: this machine's,
/// every concrete `Host` alias in an ssh config (no wildcard patterns),
/// and the extra ones given.
pub(crate) fn candidate_names(this_host: Option<&str>, ssh_config: &str, extra: &[String]) -> Vec<String> {
    let mut names: Vec<String> = this_host.map(String::from).into_iter().collect();
    for line in ssh_config.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Host ").or_else(|| line.strip_prefix("host ")) else { continue };
        names.extend(rest.split_whitespace().filter(|a| !a.contains(['*', '?', '!'])).filter_map(agentops_mcp::hooks::normalize_host));
    }
    names.extend(extra.iter().filter_map(|h| agentops_mcp::hooks::normalize_host(h)));
    let mut seen = Vec::new();
    names.retain(|n| !seen.contains(n) && {
        seen.push(n.clone());
        true
    });
    names
}

/// Every name that occurs in `text` as a whole word, case-insensitively:
/// the characters on either side aren't letters or digits, so `deploy-box`
/// matches in "on deploy-box:" but `box` doesn't match "inbox".
pub(crate) fn mentioned(text: &str, names: &[String]) -> Vec<String> {
    let text = text.to_lowercase();
    let mut found: Vec<String> = names
        .iter()
        .filter(|name| {
            text.match_indices(name.as_str()).any(|(i, _)| {
                let before = text[..i].chars().next_back();
                let after = text[i + name.len()..].chars().next();
                !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
            })
        })
        .cloned()
        .collect();
    found.sort();
    found
}

fn effect(hosts: &[String], this_host: Option<&str>) -> String {
    match this_host {
        Some(h) if hosts.iter().any(|x| x == h) => format!("error cues only on {}", hosts.join(", ")),
        _ => "error cues: never from this machine".to_string(),
    }
}

fn render(proposals: &[ScopeProposal], names: &[String], this_host: Option<&str>) -> String {
    let mut by_scope: BTreeMap<String, usize> = BTreeMap::new();
    for p in proposals {
        *by_scope.entry(p.applies_to.join(", ")).or_default() += 1;
    }
    let mut out = format!("# Proposed memory scopes\n\nThis machine: {}. Names looked for: {}.\n\n", this_host.unwrap_or("(unknown)"), names.join(", "));
    out += "Prompts, subagent seeds and file cues still show a scoped memory, labelled with its hosts; only error cues skip it on other machines.\n\n| Scope | Memories |\n| --- | --- |\n";
    for (scope, n) in &by_scope {
        out += &format!("| {scope} | {n} |\n");
    }
    out += "\n| Node | Kind | Current | Proposed | Effect | Title |\n| --- | --- | --- | --- | --- | --- |\n";
    for p in proposals {
        out += &format!("| {} | {} | {} | {} | {} | {} |\n", p.node_id, p.kind, p.current.as_ref().map(|c| c.join(", ")).unwrap_or_default(), p.applies_to.join(", "), p.effect, p.title.replace('|', "\\|"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_host_concrete_ssh_aliases_and_extras_normalized() {
        let config = "Host deploy-box db.internal\n  HostName 10.0.0.2\nHost *.example.com\nhost Laptop\nMatch all\n";
        assert_eq!(candidate_names(Some("laptop"), config, &["Extra.local".into()]), vec!["laptop", "deploy-box", "db", "extra"]);
    }

    #[test]
    fn a_name_matches_only_as_a_whole_word_whatever_its_case() {
        let names = vec!["deploy-box".to_string(), "box".to_string(), "db".to_string()];
        // A hyphen is a word boundary, so a shorter name inside a longer one
        // matches too; the review is where that gets caught.
        assert_eq!(mentioned("Logs live in PM2 on Deploy-Box: check them", &names), vec!["box", "deploy-box"]);
        assert!(mentioned("the inbox and dbx tools", &names).is_empty());
        assert_eq!(mentioned("ssh db, then box", &names), vec!["box", "db"]);
    }

    #[test]
    fn the_effect_says_whether_this_machine_still_sees_the_error_cue() {
        assert_eq!(effect(&["laptop".into()], Some("laptop")), "error cues only on laptop");
        assert_eq!(effect(&["deploy-box".into()], Some("laptop")), "error cues: never from this machine");
    }
}
