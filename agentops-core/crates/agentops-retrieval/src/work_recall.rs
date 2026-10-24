//! Cue resolution for recall during work (milestone 3b): the file the
//! agent opened or edited, or the error it just hit, becomes a ranked list
//! of candidate memories, each with a score in about 0-1 that activation
//! sums and `INJECT_MIN_SCORE` thresholds. `agentops_mcp`'s `PostToolUse`
//! hook and `agentops work-recall-eval` both call [`resolve_cue`], so the
//! evaluation measures the code the hook runs.
//!
//! A file cue's sources, in order of trust:
//! 1. memories with `Affects` edges to symbols defined in the file, scored
//!    `affects_confidence_for` how many such memories the file reaches;
//! 2. memories whose text mentions the file's repo-relative path
//!    (`MENTION_PATH_SCORE`), or its bare file name when no other file in
//!    the repo has that name (`MENTION_NAME_SCORE`);
//!
//! A memory reached both ways keeps its best score. Two more sources were
//! removed after the milestone 3b revision 1 ablation (`m3b-eval.md`),
//! since neither added a case hit: a `SimilarTo` hop from these candidates
//! (about 4k characters over five replayed sessions) and the file itself
//! as a query through prompt recall (62k characters).
//!
//! An error cue is the failed call's text, cleaned of paths, line numbers,
//! hashes and timestamps, through the same recall, keeping only what a
//! keyword or a named symbol matched: meaning and the `SimilarTo` hop may
//! re-rank those but never add one. Gotchas get `ERROR_GOTCHA_BOOST`, up
//! to 1.0. Ties break on arrival order, never on
//! node id.
//!
//! [`CueSources::AffectsOnly`] stops after source 1: what an agent gets
//! today by calling `get_symbol` on the file's symbols, and the
//! evaluation's baseline.

use std::collections::{HashMap, HashSet};

use agentops_embeddings::Embedder;
use agentops_graph::{EdgeRelation, GraphStore, NodeKind, NodeProminence};
use anyhow::Result;

use crate::recall::affects_confidence_for;
use crate::recall::recall_ranked_with;
use crate::{RecallPointer, RECALL_KINDS};

/// Score of a memory whose text names the file's repo-relative path.
pub const MENTION_PATH_SCORE: f64 = 0.9;
/// Score of a memory naming only the file's name, when that name is unique
/// in the repo (a bare `lib.rs` never counts: milestone 3a overcount).
pub const MENTION_NAME_SCORE: f64 = 0.7;
/// Longest cleaned error text sent to recall.
pub const ERROR_CUE_TEXT_MAX: usize = 300;
/// A gotcha matching an error counts this much more, up to a score of 1.0:
/// an error is when a gotcha matters most, but the boost lifts a partial
/// match rather than putting a full one above everything.
pub const ERROR_GOTCHA_BOOST: f64 = 1.5;
/// Candidates taken from each source before merging.
const PER_SOURCE: usize = 20;
/// Words that mark the important lines of an error (shared with
/// `agentops_mcp::budget`'s JSON compressor).
pub const ERROR_KEYWORDS: [&str; 5] = ["error", "fail", "exception", "panic", "fatal"];

/// What a work cue is: a file the agent read or edited (its repo-relative
/// path), or the text of a failed tool call.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorkCue {
    File {
        path: String,
    },
    Error {
        text: String,
    },
}

/// Which cue sources run. `AffectsOnly` is the baseline; each `No*` turns
/// one file-cue source off, for the ablation (milestone 3b revision 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CueSources {
    #[default]
    All,
    AffectsOnly,
    /// Without source 1, the file's `Affects`-linked memories.
    NoAffects,
    /// Without source 2, path mentions.
    NoMentions,
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

/// Candidates for `cue`, best score first (then id). A skipped path has
/// none; an error cue has none under `AffectsOnly`. `embedder` lets
/// recall by meaning re-rank an error cue's keyword matches; only the
/// server passes one (ADR 0052).
pub fn resolve_cue(store: &dyn GraphStore, repo: &str, cue: &WorkCue, sources: CueSources, embedder: Option<&'static (dyn Embedder + Sync)>) -> Result<Vec<WorkCandidate>> {
    let mut merged = Merged::default();
    match cue {
        WorkCue::File { path } if is_skipped(path) => {}
        WorkCue::File { path } => {
            let symbols: Vec<agentops_graph::Node> = store.nodes_in_file(repo, path)?.into_iter().filter(|n| n.kind == NodeKind::Symbol).collect();
            if sources != CueSources::NoAffects {
                merged.extend(affects_candidates(store, repo, &symbols)?);
            }
            if !matches!(sources, CueSources::AffectsOnly | CueSources::NoMentions) {
                merged.extend(mention_candidates(store, repo, path)?);
            }
        }
        WorkCue::Error { .. } if sources == CueSources::AffectsOnly => {}
        WorkCue::Error { text } => {
            let query = clean_error(text);
            if query.trim().is_empty() {
                return Ok(Vec::new());
            }
            // Only what a keyword (or a named symbol) matched: meaning may
            // re-rank those but never adds one, since a self-explanatory
            // error otherwise pulled in unrelated memories by meaning alone
            // (gotcha 235643, milestone 3b revision 1).
            merged.extend(recall_ranked_with(store, repo, &query, PER_SOURCE, &HashSet::new(), embedder)?.into_iter().filter(|s| s.lexical).map(|s| {
                let score = if s.pointer.kind == NodeKind::Gotcha { (s.confidence * ERROR_GOTCHA_BOOST).min(1.0).max(s.confidence) } else { s.confidence };
                WorkCandidate { pointer: s.pointer, score }
            }));
        }
    }
    Ok(merged.sorted())
}

/// Cue events activation looks back over: roughly the current stretch of
/// work.
pub const ACTIVATION_WINDOW: usize = 20;
/// What a cue's contribution keeps per later cue: one 10 cues back counts
/// about 20%.
pub const ACTIVATION_DECAY: f64 = 0.85;

/// Session activation (milestone 3b, ADR 0053): for each memory, the sum
/// over the last `window` cues (oldest first in `cues`, each a list of
/// `(node id, score)`) of its score times `decay` raised to the cue's age,
/// the newest cue being age 0. A memory several recent cues reached
/// outranks one a single old cue did.
pub fn activation(cues: &[Vec<(i64, f64)>], window: usize, decay: f64) -> HashMap<i64, f64> {
    let mut out: HashMap<i64, f64> = HashMap::new();
    for (age, cue) in cues.iter().rev().take(window).enumerate() {
        let weight = decay.powi(age as i32);
        for &(id, score) in cue {
            *out.entry(id).or_default() += score * weight;
        }
    }
    out
}

/// Candidates by node id, each keeping its best score and the reason that
/// came with it (the first source wins a tie).
/// Ties break on arrival order: sources in order of trust, each in its own
/// rank order (recall's ranking for the query sources), so a tie never
/// falls back to node id.
#[derive(Default)]
struct Merged(HashMap<i64, (usize, WorkCandidate)>);

impl Merged {
    fn extend(&mut self, candidates: impl IntoIterator<Item = WorkCandidate>) {
        for c in candidates {
            let arrival = self.0.len();
            match self.0.get_mut(&c.pointer.node_id) {
                Some((_, existing)) if c.score > existing.score => *existing = c,
                Some(_) => {}
                None => {
                    self.0.insert(c.pointer.node_id, (arrival, c));
                }
            }
        }
    }

    fn sorted(&self) -> Vec<WorkCandidate> {
        let mut out: Vec<&(usize, WorkCandidate)> = self.0.values().collect();
        out.sort_by(|(ia, a), (ib, b)| b.score.total_cmp(&a.score).then(ia.cmp(ib)));
        out.into_iter().map(|(_, c)| c.clone()).collect()
    }
}

/// Whether recall may point at `node`: a memory, not pinned (the pin block
/// shows those), not needs-review, and allowed by its class (ADR 0054).
fn recallable(node: &agentops_graph::Node, today: &str) -> bool {
    RECALL_KINDS.contains(&node.kind) && node.prominence.recallable() && node.prominence != NodeProminence::Pinned && node.memory_class.recallable_at(today, node.class_until.as_deref())
}

fn candidate(node: agentops_graph::Node, score: f64, reason: String) -> WorkCandidate {
    let score = if node.prominence == NodeProminence::Reduced { score * 0.5 } else { score };
    WorkCandidate { pointer: RecallPointer { node_id: node.id, kind: node.kind, title: node.name.unwrap_or_else(|| "(untitled)".to_string()), reason, memory_class: node.memory_class, applies_to: node.applies_to }, score }
}

/// Source 1: memories with an `Affects` edge to one of `symbols` (the
/// file's), each naming the symbols it affects.
fn affects_candidates(store: &dyn GraphStore, repo: &str, symbols: &[agentops_graph::Node]) -> Result<Vec<WorkCandidate>> {
    let mut affected: HashMap<i64, Vec<String>> = HashMap::new();
    let mut order = Vec::new();
    for symbol in symbols {
        let name = symbol.name.clone().unwrap_or_default();
        for edge in store.edges_to(repo, symbol.id)? {
            if edge.relation != EdgeRelation::Affects {
                continue;
            }
            let names = affected.entry(edge.src_id).or_insert_with(|| {
                order.push(edge.src_id);
                Vec::new()
            });
            if !names.contains(&name) {
                names.push(name.clone());
            }
        }
    }
    let score = affects_confidence_for(order.len());
    let today = agentops_graph::utc_today();
    let mut out = Vec::new();
    for id in order {
        let Some(note) = store.get_node(repo, id)? else { continue };
        if !recallable(&note, &today) {
            continue;
        }
        let reason = format!("affects {}", affected[&id].iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", "));
        out.push(candidate(note, score, reason));
    }
    Ok(out)
}

/// Source 2: memories whose text names `path`, or its file name when that
/// name is unique among the repo's files.
fn mention_candidates(store: &dyn GraphStore, repo: &str, path: &str) -> Result<Vec<WorkCandidate>> {
    let today = agentops_graph::utc_today();
    let mut out: Vec<WorkCandidate> = store.nodes_mentioning(repo, path, RECALL_KINDS, PER_SOURCE)?.into_iter().filter(|n| recallable(n, &today)).map(|n| candidate(n, MENTION_PATH_SCORE, format!("mentions `{path}`"))).collect();
    let name = path.rsplit('/').next().unwrap_or(path);
    if name != path && store.count_files_named(repo, name)? == 1 {
        let seen: HashSet<i64> = out.iter().map(|c| c.pointer.node_id).collect();
        out.extend(store.nodes_mentioning(repo, name, RECALL_KINDS, PER_SOURCE)?.into_iter().filter(|n| recallable(n, &today) && !seen.contains(&n.id)).map(|n| candidate(n, MENTION_NAME_SCORE, format!("mentions `{name}`"))));
    }
    Ok(out)
}

/// `line` without a leading `Exit code N` (Bash's first line, or the
/// start of a flattened error).
fn strip_exit_code(line: &str) -> &str {
    match line.strip_prefix("Exit code") {
        Some(rest) => rest.trim_start().trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches([' ', '|']),
        None => line,
    }
}

/// An error's text as a recall query: its error lines first (those with
/// an `ERROR_KEYWORDS` word), then the rest. Paths are cut to their file
/// name, and line numbers, hex hashes, timestamps and bare numbers are
/// dropped, at most `ERROR_CUE_TEXT_MAX` characters. The tool's
/// `<tool_use_error>` wrapper and `Exit code N` line say nothing.
pub fn clean_error(text: &str) -> String {
    let clean_line = |line: &str| -> String {
        line.replace("<tool_use_error>", " ")
            .replace("</tool_use_error>", " ")
            .split_whitespace()
            .filter_map(|word| {
                let word = if word.contains('/') { word.trim_end_matches(['/', ':', ',', '.', ')', '\'']).rsplit('/').next().unwrap_or("") } else { word };
                // `file.rs:7:17` keeps `file.rs`.
                let word = word.trim_end_matches(|c: char| c.is_ascii_digit() || c == ':');
                let core = word.trim_matches(|c: char| !c.is_alphanumeric());
                let numeric = core.chars().all(|c| c.is_ascii_digit() || matches!(c, ':' | '.' | '-' | 'T' | 'Z'));
                let hex = core.len() >= 7 && core.chars().all(|c| c.is_ascii_hexdigit());
                (!core.is_empty() && !numeric && !hex).then_some(word)
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let lines: Vec<String> = text.lines().map(|l| strip_exit_code(l.trim_start())).map(clean_line).filter(|l| !l.is_empty()).collect();
    let is_error = |l: &String| {
        let lower = l.to_lowercase();
        ERROR_KEYWORDS.iter().any(|k| lower.contains(k))
    };
    let ordered: Vec<&String> = lines.iter().filter(|l| is_error(l)).chain(lines.iter().filter(|l| !is_error(l))).collect();
    let joined = ordered.iter().map(|l| l.as_str()).collect::<Vec<_>>().join(" ");
    joined.chars().take(ERROR_CUE_TEXT_MAX).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{upsert_node, MemoryClass, NewNode, SqliteGraphStore};

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
        assert!(resolve_cue(&store, "r", &WorkCue::File { path: "target/gen.rs".into() }, CueSources::All, None).unwrap().is_empty());
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

    fn memory(store: &SqliteGraphStore, kind: NodeKind, name: &str, content: &str) -> i64 {
        upsert_node(store, NewNode { kind, repo: "r".into(), path: Some(format!("vault:{name}")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(content.into()) }).unwrap()
    }

    fn file(store: &SqliteGraphStore, path: &str) {
        upsert_node(store, NewNode { kind: NodeKind::File, repo: "r".into(), path: Some(path.into()), name: None, container: None, start_line: None, end_line: None, content: None }).unwrap();
    }

    fn ids(found: &[WorkCandidate]) -> Vec<i64> {
        found.iter().map(|c| c.pointer.node_id).collect()
    }

    #[test]
    fn a_doc_with_no_affects_link_gets_candidates_from_a_path_mention() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        file(&store, "docs/release.md");
        let target = memory(&store, NodeKind::Gotcha, "Tagging a release needs the changelog bumped first", "Release tagging fails unless docs/release.md has the changelog entry.");
        memory(&store, NodeKind::Gotcha, "Unrelated", "Webhooks retry three times.");
        let cue = WorkCue::File { path: "docs/release.md".into() };
        assert!(resolve_cue(&store, "r", &cue, CueSources::AffectsOnly, None).unwrap().is_empty(), "no Affects link");
        let found = resolve_cue(&store, "r", &cue, CueSources::All, None).unwrap();
        assert_eq!(ids(&found).first(), Some(&target), "{found:?}");
        assert!(found[0].score > 0.0 && found[0].score <= 1.0);
    }

    #[test]
    fn a_file_name_shared_by_two_files_never_counts_as_a_mention() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        for path in ["a/src/lib.rs", "b/src/lib.rs", "a/src/hooks.rs"] {
            file(&store, path);
        }
        let shared = memory(&store, NodeKind::Gotcha, "g1", "Careful in lib.rs when exporting.");
        let full = memory(&store, NodeKind::Gotcha, "g2", "a/src/lib.rs re-exports everything.");
        let unique = memory(&store, NodeKind::Gotcha, "g3", "hooks.rs must never load the model.");
        let lib = mention_candidates(&store, "r", "a/src/lib.rs").unwrap();
        assert_eq!(ids(&lib), [full], "the full path counts, the shared name doesn't");
        assert_eq!(lib[0].score, MENTION_PATH_SCORE);
        let hooks = mention_candidates(&store, "r", "a/src/hooks.rs").unwrap();
        assert_eq!(ids(&hooks), [unique]);
        assert_eq!(hooks[0].score, MENTION_NAME_SCORE);
        let _ = shared;
    }

    #[test]
    fn an_error_is_cleaned_to_its_telling_words_and_boosts_gotchas() {
        assert_eq!(clean_error("Exit code 7 | curl: (7) Failed to connect"), "curl Failed to connect");
        assert_eq!(clean_error("Exit code 1"), "");
        let cleaned = clean_error("Exit code 1\nwarning: unused import at /Users/x/repo/src/main.rs:7:17\nerror[E0425]: cannot find value `foo` in this scope, commit 9db16a7c, at 2026-10-20T18:13:38");
        assert!(cleaned.starts_with("error[E0425] cannot find value `foo`"), "{cleaned}");
        assert!(cleaned.contains("main.rs"), "a path keeps its file name: {cleaned}");
        for gone in ["Exit code", "/Users", "9db16a7c", "2026-10-20", "7:17"] {
            assert!(!cleaned.contains(gone), "{gone} in {cleaned}");
        }

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let gotcha = memory(&store, NodeKind::Gotcha, "Postgres role missing", "psql fails with role postgres does not exist on the CI box.");
        let decision = memory(&store, NodeKind::Decision, "Postgres role naming", "The CI role is named agentops, not postgres; psql role exists.");
        let found = resolve_cue(&store, "r", &WorkCue::Error { text: "psql: error: FATAL: role \"postgres\" does not exist".into() }, CueSources::All, None).unwrap();
        let score = |id| found.iter().find(|c| c.pointer.node_id == id).map(|c| c.score);
        assert!(score(gotcha).unwrap() > score(decision).unwrap_or(0.0), "{found:?}");
        assert!(resolve_cue(&store, "r", &WorkCue::Error { text: "psql: error".into() }, CueSources::AffectsOnly, None).unwrap().is_empty());
    }

    #[test]
    fn each_ablation_switch_drops_exactly_its_source() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let path = "src/billing_ledger.rs";
        let symbol = node(&store, NodeKind::Symbol, path, "reconcile");
        let affects = memory(&store, NodeKind::Decision, "Entries are append-only", "Never update entries once written.");
        store.add_edge("r", affects, symbol, EdgeRelation::Affects).unwrap();
        let mention = memory(&store, NodeKind::Gotcha, "Rounding in one place", "src/billing_ledger.rs rounds once, at posting.");
        let cue = WorkCue::File { path: path.into() };
        let reached = |sources| -> HashSet<i64> { resolve_cue(&store, "r", &cue, sources, None).unwrap().into_iter().map(|c| c.pointer.node_id).collect() };
        let all = reached(CueSources::All);
        assert!([affects, mention].iter().all(|id| all.contains(id)), "each source reaches its memory: {all:?}");
        for (sources, gone) in [(CueSources::NoAffects, vec![affects]), (CueSources::NoMentions, vec![mention])] {
            let without = reached(sources);
            let expected: HashSet<i64> = all.iter().copied().filter(|id| !gone.contains(id)).collect();
            assert_eq!(without, expected, "{sources:?}");
        }
    }

    #[test]
    fn an_error_keeps_only_what_a_keyword_matched() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let matched = memory(&store, NodeKind::Gotcha, "Postgres role missing", "psql fails with role postgres does not exist on the CI box.");
        let neighbour = memory(&store, NodeKind::Note, "CI database bootstrap order", "Seed users before migrations.");
        let mut v = vec![0.0; agentops_embeddings::EMBEDDING_DIM];
        v[0] = 1.0;
        store.set_embedding("r", matched, &v).unwrap();
        store.set_embedding("r", neighbour, &v).unwrap();
        store.add_edge("r", matched, neighbour, EdgeRelation::SimilarTo).unwrap();
        store.add_edge("r", neighbour, matched, EdgeRelation::SimilarTo).unwrap();
        let prompt_time = crate::recall_scored_with(&store, "r", "psql role postgres does not exist", 8, &HashSet::new(), None).unwrap();
        assert!(prompt_time.iter().any(|(p, _)| p.node_id == neighbour), "prompt recall still hops: {prompt_time:?}");
        let found = resolve_cue(&store, "r", &WorkCue::Error { text: "psql: error: FATAL: role \"postgres\" does not exist".into() }, CueSources::All, None).unwrap();
        assert_eq!(found.iter().map(|c| c.pointer.node_id).collect::<Vec<_>>(), [matched], "a hop neighbour never rides in on an error: {found:?}");
    }

    /// ADR 0054: a progress memory never reaches recall, not even from a
    /// file it's `Affects`-linked to; a temporal one only before its date;
    /// an experimental one always, labelled.
    #[test]
    fn each_memory_class_reaches_only_the_cues_its_rule_allows() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let path = "src/billing_ledger.rs";
        let symbol = node(&store, NodeKind::Symbol, path, "reconcile");
        let status = memory(&store, NodeKind::Note, "Ledger reconcile shipped", "Ledger reconcile shipped; src/billing_ledger.rs postgres role check added.");
        store.add_edge("r", status, symbol, EdgeRelation::Affects).unwrap();
        let mentioned_status = memory(&store, NodeKind::Note, "Ledger export shipped", "Export from src/billing_ledger.rs shipped.");
        let expired = memory(&store, NodeKind::Gotcha, "Ledger rounding workaround", "src/billing_ledger.rs rounds twice until the fix.");
        let experimental = memory(&store, NodeKind::Decision, "Try posting in batches", "src/billing_ledger.rs posts in batches for now.");
        store.set_memory_class("r", status, MemoryClass::Progress, None).unwrap();
        store.set_memory_class("r", mentioned_status, MemoryClass::Progress, None).unwrap();
        store.set_memory_class("r", expired, MemoryClass::Temporal, Some("2020-01-01")).unwrap();
        store.set_memory_class("r", experimental, MemoryClass::Experimental, None).unwrap();

        let found = resolve_cue(&store, "r", &WorkCue::File { path: path.into() }, CueSources::All, None).unwrap();
        let reached = ids(&found);
        assert!(!reached.contains(&status), "not even from the file it affects: {found:?}");
        assert!(!reached.contains(&mentioned_status), "nor through a mention");
        assert!(!reached.contains(&expired), "a temporal memory past its date is gone");
        let marked = found.iter().find(|c| c.pointer.node_id == experimental).expect("experimental is shown");
        assert!(marked.pointer.render_short().contains("Try posting in batches — experimental (id"), "{}", marked.pointer.render_short());

        let error = resolve_cue(&store, "r", &WorkCue::Error { text: "ledger reconcile shipped postgres role check".into() }, CueSources::All, None).unwrap();
        assert!(!ids(&error).contains(&status), "never from an error: {error:?}");
        let prompt = crate::recall_scored_with(&store, "r", "ledger reconcile shipped postgres role check", 8, &HashSet::new(), None).unwrap();
        assert!(prompt.iter().all(|(p, _)| p.node_id != status), "never from a prompt: {prompt:?}");
    }

    #[test]
    fn a_memory_three_recent_cues_reached_outranks_one_a_single_cue_reached_ten_cues_ago() {
        let (recent, old) = (1, 2);
        let mut cues: Vec<Vec<(i64, f64)>> = vec![vec![(old, 1.0)]];
        cues.extend(std::iter::repeat_n(vec![(99, 0.1)], 6));
        cues.extend([vec![(recent, 0.5)], vec![], vec![(recent, 0.5)], vec![(recent, 0.5)]]);
        let a = activation(&cues, ACTIVATION_WINDOW, ACTIVATION_DECAY);
        assert!(a[&recent] > a[&old], "{a:?}");
        assert!((a[&old] - ACTIVATION_DECAY.powi(10)).abs() < 1e-9, "ten cues back: {}", a[&old]);
        let windowed = activation(&cues, 5, ACTIVATION_DECAY);
        assert!(!windowed.contains_key(&old), "outside the window");
    }

    #[test]
    fn an_affects_link_counts_less_the_more_memories_the_file_reaches() {
        assert_eq!(affects_confidence_for(1), 1.0);
        assert!((affects_confidence_for(crate::recall::AFFECTS_HUB) - 0.5).abs() < 1e-9);
        assert!(affects_confidence_for(50) < 0.45 && affects_confidence_for(50) > 0.4);
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
        let got = resolve_cue(&store, "r", &WorkCue::File { path: "src/a.rs".into() }, CueSources::AffectsOnly, None).unwrap();
        assert_eq!(got.iter().map(|c| c.pointer.node_id).collect::<Vec<_>>(), vec![both, one]);
        assert_eq!(got[0].pointer.reason, "affects `f`, `g`");
    }
}
