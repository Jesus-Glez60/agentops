//! Cue resolution for recall during work (milestone 3b): the file the
//! agent opened or edited, or the error it just hit, becomes a ranked list
//! of candidate memories. `agentops_mcp`'s `PostToolUse` hook and
//! `agentops work-recall-eval` both call it, so the evaluation measures the
//! code the hook runs.
//!
//! A file cue's sources, in order of trust:
//! 1. memories with `Affects` edges to symbols defined in the file;
//! 2. (item 2) memories whose text mentions the file's repo-relative path;
//! 3. (item 2) the file itself as a query: path, first heading, symbols.
//!
//! [`CueSources::AffectsOnly`] stops after source 1: what an agent gets
//! today by calling `get_symbol` on the file's symbols, and the
//! evaluation's baseline.

use std::collections::HashMap;

use agentops_graph::{EdgeRelation, GraphStore, NodeKind};
use anyhow::Result;

use crate::{RecallPointer, RECALL_KINDS};

/// What a work cue is: a file the agent read or edited (repo-relative
/// path, plus the first heading of its content when the hook saw it), or
/// the text of a failed tool call.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorkCue {
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heading: Option<String>,
    },
    Error {
        text: String,
    },
}

/// Which cue sources run. `AffectsOnly` is the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CueSources {
    #[default]
    All,
    AffectsOnly,
}

/// A memory a cue reached, with the score activation sums (item 3).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkCandidate {
    pub pointer: RecallPointer,
    pub score: f64,
}

/// Paths that are never a cue: version control, AgentOps' own state, build
/// output, dependencies, lockfiles. Matched as a path prefix, a path
/// segment, or a file name.
pub const WORK_CUE_SKIP: &[&str] = &[".git/", ".context/", "target/", "node_modules/", ".next/", "Cargo.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"];

/// Whether `path` (repo-relative) is on the skip list.
pub fn is_skipped(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    WORK_CUE_SKIP.iter().any(|skip| match skip.strip_suffix('/') {
        Some(dir) => path.starts_with(skip) || path.contains(&format!("/{dir}/")),
        None => path == *skip || path.ends_with(&format!("/{skip}")),
    })
}

/// The file a Bash command only reads, if it's one of the simple shapes
/// agents use in place of the Read tool: `cat`, `head`, `tail`, `nl` or
/// `sed -n '…p'` on exactly one file, optionally after `cd <dir> &&` (or
/// `;`). The path comes back joined to that `cd` directory when relative;
/// the caller makes it repo-relative. Anything chained, piped, redirected
/// or substituted is not a read (milestone 3b decision: recent sessions read
/// mostly this way, 4-12 Read tool calls each).
pub fn bash_read_path(command: &str) -> Option<String> {
    let command = command.trim();
    let (dir, rest) = match command.strip_prefix("cd ") {
        Some(after) => {
            let (dir, rest) = after.split_once("&&").or_else(|| after.split_once(';'))?;
            (Some(single_word(dir.trim())?), rest.trim())
        }
        None => (None, command),
    };
    if rest.contains(['|', '>', '<', ';', '&', '`', '\n']) || rest.contains("$(") {
        return None;
    }
    let words = shell_words(rest)?;
    let (program, args) = words.split_first()?;
    let files: Vec<&String> = match program.as_str() {
        "cat" | "nl" => args.iter().filter(|a| !a.starts_with('-')).collect(),
        "head" | "tail" => {
            let mut files = Vec::new();
            let mut skip_value = false;
            for a in args {
                if skip_value {
                    skip_value = false;
                } else if a == "-n" || a == "-c" {
                    skip_value = true;
                } else if !a.starts_with('-') {
                    files.push(a);
                }
            }
            files
        }
        "sed" => {
            // Only `sed -n '<range>p' <file>`: a print, never an edit.
            let [flag, script, file] = args else { return None };
            if flag != "-n" || !script.ends_with('p') || script.contains(['s', 'w', 'e']) {
                return None;
            }
            vec![file]
        }
        _ => return None,
    };
    let [file] = files[..] else { return None };
    Some(match dir {
        Some(dir) if !file.starts_with('/') => format!("{}/{file}", dir.trim_end_matches('/')),
        _ => file.clone(),
    })
}

/// `command` split into words, single- and double-quoted spans kept whole
/// with their quotes removed. `None` for an unbalanced quote, or an
/// unquoted glob or variable, which can't be read as a literal path.
fn shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None if matches!(c, '*' | '?' | '$') => return None,
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

/// `text` as exactly one shell word.
fn single_word(text: &str) -> Option<String> {
    match shell_words(text)?.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// Candidates for `cue`, best first. A skipped path, or an error cue
/// under `AffectsOnly`, has none.
pub fn resolve_cue(store: &dyn GraphStore, repo: &str, cue: &WorkCue, _sources: CueSources) -> Result<Vec<WorkCandidate>> {
    match cue {
        WorkCue::File { path, .. } if is_skipped(path) => Ok(Vec::new()),
        WorkCue::File { path, .. } => affects_candidates(store, repo, path),
        // An error names no file, so `AffectsOnly` has nothing to follow;
        // error recall arrives with item 2.
        WorkCue::Error { .. } => Ok(Vec::new()),
    }
}

/// Source 1: memories with an `Affects` edge to a symbol defined in
/// `path`. A memory affecting several of the file's symbols scores once
/// per symbol; ties break on id so the order never depends on map order.
fn affects_candidates(store: &dyn GraphStore, repo: &str, path: &str) -> Result<Vec<WorkCandidate>> {
    let mut found: HashMap<i64, (WorkCandidate, Vec<String>)> = HashMap::new();
    for symbol in store.nodes_in_file(repo, path)?.into_iter().filter(|n| n.kind == NodeKind::Symbol) {
        let name = symbol.name.clone().unwrap_or_default();
        for edge in store.edges_to(repo, symbol.id)? {
            if edge.relation != EdgeRelation::Affects {
                continue;
            }
            if let Some((candidate, symbols)) = found.get_mut(&edge.src_id) {
                candidate.score += 1.0;
                if !symbols.contains(&name) {
                    symbols.push(name.clone());
                }
                continue;
            }
            let Some(note) = store.get_node(repo, edge.src_id)? else { continue };
            if !RECALL_KINDS.contains(&note.kind) || !note.prominence.recallable() || note.prominence == agentops_graph::NodeProminence::Pinned {
                continue;
            }
            let pointer = RecallPointer { node_id: note.id, kind: note.kind, title: note.name.unwrap_or_else(|| "(untitled)".to_string()), reason: String::new() };
            found.insert(note.id, (WorkCandidate { pointer, score: 1.0 }, vec![name.clone()]));
        }
    }
    let mut out: Vec<WorkCandidate> = found
        .into_values()
        .map(|(mut c, symbols)| {
            c.pointer.reason = format!("affects {}", symbols.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", "));
            c
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.pointer.node_id.cmp(&b.pointer.node_id)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{upsert_node, NewNode, SqliteGraphStore};

    fn node(store: &SqliteGraphStore, kind: NodeKind, path: &str, name: &str) -> i64 {
        upsert_node(store, NewNode { kind, repo: "r".into(), path: Some(path.into()), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(name.into()) }).unwrap()
    }

    #[test]
    fn skip_list_paths_never_produce_a_cue() {
        for path in [".git/HEAD", ".context/graph.db", "target/debug/x", "apps/web/node_modules/a/b.js", "Cargo.lock", "apps/web/package-lock.json", "crates/x/target/y"] {
            assert!(is_skipped(path), "{path}");
        }
        for path in ["src/lib.rs", "docs/target-audience.md", "Cargo.toml", "contextual.rs"] {
            assert!(!is_skipped(path), "{path}");
        }
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let gotcha = node(&store, NodeKind::Gotcha, "vault:g", "g");
        let symbol = node(&store, NodeKind::Symbol, "target/gen.rs", "f");
        store.add_edge("r", gotcha, symbol, EdgeRelation::Affects).unwrap();
        assert!(resolve_cue(&store, "r", &WorkCue::File { path: "target/gen.rs".into(), heading: None }, CueSources::All).unwrap().is_empty());
    }

    #[test]
    fn a_simple_single_file_bash_read_names_its_file() {
        assert_eq!(bash_read_path("cat src/a.rs").as_deref(), Some("src/a.rs"));
        assert_eq!(bash_read_path("sed -n 1,60p 'docs/x y.md'").as_deref(), Some("docs/x y.md"));
        assert_eq!(bash_read_path("head -n 40 \"/abs/b.rs\"").as_deref(), Some("/abs/b.rs"));
        assert_eq!(bash_read_path("cd crates && sed -n 10,20p agentops-mcp/src/hooks.rs").as_deref(), Some("crates/agentops-mcp/src/hooks.rs"));
        assert_eq!(bash_read_path("cd /repo; tail -50 /etc/hosts").as_deref(), Some("/etc/hosts"));
        for not_a_read in ["cat a.rs b.rs", "cat a.rs | grep x", "sed -i 's/a/b/' a.rs", "sed -n 's/a/b/p' a.rs", "cat *.rs", "cat $F", "cat a > b", "grep -n x a.rs", "cd x && cat a && cat b", "head"] {
            assert_eq!(bash_read_path(not_a_read), None, "{not_a_read}");
        }
    }

    #[test]
    fn affects_only_finds_the_notes_on_a_files_symbols_strongest_first() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let f = node(&store, NodeKind::Symbol, "src/a.rs", "f");
        let g = node(&store, NodeKind::Symbol, "src/a.rs", "g");
        let elsewhere = node(&store, NodeKind::Symbol, "src/b.rs", "h");
        let both = node(&store, NodeKind::Gotcha, "vault:both", "both");
        let one = node(&store, NodeKind::Decision, "vault:one", "one");
        let other = node(&store, NodeKind::Gotcha, "vault:other", "other");
        store.add_edge("r", both, f, EdgeRelation::Affects).unwrap();
        store.add_edge("r", both, g, EdgeRelation::Affects).unwrap();
        store.add_edge("r", one, g, EdgeRelation::Affects).unwrap();
        store.add_edge("r", other, elsewhere, EdgeRelation::Affects).unwrap();
        let got = resolve_cue(&store, "r", &WorkCue::File { path: "src/a.rs".into(), heading: None }, CueSources::AffectsOnly).unwrap();
        assert_eq!(got.iter().map(|c| c.pointer.node_id).collect::<Vec<_>>(), vec![both, one]);
        assert_eq!(got[0].pointer.reason, "affects `f`, `g`");
    }
}
