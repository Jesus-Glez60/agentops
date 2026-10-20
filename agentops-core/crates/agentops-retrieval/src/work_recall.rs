//! Cue resolution for recall during work (milestone 3b): the file the
//! agent opened or edited, or the error it just hit, becomes a ranked list
//! of candidate memories, each with a score in about 0-1 that activation
//! sums and `INJECT_MIN_SCORE` thresholds. `agentops_mcp`'s `PostToolUse`
//! hook and `agentops work-recall-eval` both call [`resolve_cue`], so the
//! evaluation measures the code the hook runs.
//!
//! A file cue's sources, in order of trust:
//! 1. memories with `Affects` edges to symbols defined in the file
//!    (score `AFFECTS_SCORE`);
//! 2. memories whose text mentions the file's repo-relative path
//!    (`MENTION_PATH_SCORE`), or its bare file name when no other file in
//!    the repo has that name (`MENTION_NAME_SCORE`);
//! 3. the file itself as a query: its name, parent directory, first
//!    heading and top-level symbols, at most `FILE_CUE_TEXT_MAX`
//!    characters, through prompt recall (`recall_scored_with`: keywords
//!    and graph locally, plus meaning on the server). Its score is recall's
//!    confidence.
//!
//! Sources 1 and 2 then get the prompt seed's `SimilarTo` hop: at most
//! `HOP_MAX_PER_SOURCE` neighbours each, scored source × cosine ×
//! `HOP_FACTOR`. Source 3 already hops inside recall. A memory reached
//! several ways keeps its best score.
//!
//! An error cue is the failed call's text, cleaned of paths, line numbers,
//! hashes and timestamps, through the same recall. Gotchas get
//! `ERROR_GOTCHA_BOOST`.
//!
//! [`CueSources::AffectsOnly`] stops after source 1: what an agent gets
//! today by calling `get_symbol` on the file's symbols, and the
//! evaluation's baseline.

use std::collections::{HashMap, HashSet};

use agentops_embeddings::Embedder;
use agentops_graph::{EdgeRelation, GraphStore, NodeKind, NodeProminence};
use anyhow::Result;

use crate::recall::{closest_similar, HOP_MAX_PER_SOURCE};
use crate::{recall_scored_with, RecallPointer, HOP_FACTOR, RECALL_KINDS};

/// Score of a memory `Affects`-linked to a symbol in the file.
pub const AFFECTS_SCORE: f64 = 1.0;
/// Score of a memory whose text names the file's repo-relative path.
pub const MENTION_PATH_SCORE: f64 = 0.9;
/// Score of a memory naming only the file's name, when that name is unique
/// in the repo (a bare `lib.rs` never counts: milestone 3a overcount).
pub const MENTION_NAME_SCORE: f64 = 0.7;
/// Longest file-as-query text: path, heading and a few symbol names.
pub const FILE_CUE_TEXT_MAX: usize = 300;
/// Longest cleaned error text sent to recall.
pub const ERROR_CUE_TEXT_MAX: usize = 300;
/// A gotcha matching an error counts this much more: an error is when a
/// gotcha matters most.
pub const ERROR_GOTCHA_BOOST: f64 = 1.5;
/// Candidates taken from each source before merging.
const PER_SOURCE: usize = 20;
/// Top-level symbol names a file-as-query includes.
const FILE_CUE_SYMBOLS: usize = 6;
/// Directory names too generic to say anything about a file.
const GENERIC_DIRS: &[&str] = &["src", "lib", "crates", "apps", "app", "components", "packages", "tests", "test", "bin", "agentops-core", "docs"];
/// Words that mark the important lines of an error (shared with
/// `agentops_mcp::budget`'s JSON compressor).
pub const ERROR_KEYWORDS: [&str; 5] = ["error", "fail", "exception", "panic", "fatal"];

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

/// Candidates for `cue`, best score first (then id). A skipped path has
/// none; an error cue has none under `AffectsOnly`. `embedder` adds
/// recall by meaning to the query-based sources; only the server passes
/// one (ADR 0052).
pub fn resolve_cue(store: &dyn GraphStore, repo: &str, cue: &WorkCue, sources: CueSources, embedder: Option<&'static (dyn Embedder + Sync)>) -> Result<Vec<WorkCandidate>> {
    let mut merged = Merged::default();
    match cue {
        WorkCue::File { path, .. } if is_skipped(path) => {}
        WorkCue::File { path, heading } => {
            let symbols: Vec<agentops_graph::Node> = store.nodes_in_file(repo, path)?.into_iter().filter(|n| n.kind == NodeKind::Symbol).collect();
            let direct = affects_candidates(store, repo, &symbols)?;
            merged.extend(direct.iter().cloned());
            if sources == CueSources::All {
                let mentions = mention_candidates(store, repo, path)?;
                merged.extend(mentions.iter().cloned());
                let sourced: Vec<WorkCandidate> = merged.sorted();
                merged.extend(hop_candidates(store, repo, &sourced)?);
                let query = file_query(path, heading.as_deref(), &symbols);
                merged.extend(recall_scored_with(store, repo, &query, PER_SOURCE, &HashSet::new(), embedder)?.into_iter().map(|(pointer, score)| WorkCandidate { pointer, score }));
            }
        }
        WorkCue::Error { .. } if sources == CueSources::AffectsOnly => {}
        WorkCue::Error { text } => {
            let query = clean_error(text);
            merged.extend(recall_scored_with(store, repo, &query, PER_SOURCE, &HashSet::new(), embedder)?.into_iter().map(|(pointer, score)| {
                let score = if pointer.kind == NodeKind::Gotcha { score * ERROR_GOTCHA_BOOST } else { score };
                WorkCandidate { pointer, score }
            }));
        }
    }
    Ok(merged.sorted())
}

/// Candidates by node id, each keeping its best score and the reason that
/// came with it (the first source wins a tie).
#[derive(Default)]
struct Merged(HashMap<i64, WorkCandidate>);

impl Merged {
    fn extend(&mut self, candidates: impl IntoIterator<Item = WorkCandidate>) {
        for c in candidates {
            match self.0.get_mut(&c.pointer.node_id) {
                Some(existing) if c.score > existing.score => *existing = c,
                Some(_) => {}
                None => {
                    self.0.insert(c.pointer.node_id, c);
                }
            }
        }
    }

    fn sorted(&self) -> Vec<WorkCandidate> {
        let mut out: Vec<WorkCandidate> = self.0.values().cloned().collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.pointer.node_id.cmp(&b.pointer.node_id)));
        out
    }
}

/// Whether recall may point at `node`: a memory, not pinned (the pin block
/// shows those), and not needs-review.
fn recallable(node: &agentops_graph::Node) -> bool {
    RECALL_KINDS.contains(&node.kind) && node.prominence.recallable() && node.prominence != NodeProminence::Pinned
}

fn candidate(node: agentops_graph::Node, score: f64, reason: String) -> WorkCandidate {
    let score = if node.prominence == NodeProminence::Reduced { score * 0.5 } else { score };
    WorkCandidate { pointer: RecallPointer { node_id: node.id, kind: node.kind, title: node.name.unwrap_or_else(|| "(untitled)".to_string()), reason }, score }
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
    let mut out = Vec::new();
    for id in order {
        let Some(note) = store.get_node(repo, id)? else { continue };
        if !recallable(&note) {
            continue;
        }
        let reason = format!("affects {}", affected[&id].iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", "));
        out.push(candidate(note, AFFECTS_SCORE, reason));
    }
    Ok(out)
}

/// Source 2: memories whose text names `path`, or its file name when that
/// name is unique among the repo's files.
fn mention_candidates(store: &dyn GraphStore, repo: &str, path: &str) -> Result<Vec<WorkCandidate>> {
    let mut out: Vec<WorkCandidate> = store.nodes_mentioning(repo, path, RECALL_KINDS, PER_SOURCE)?.into_iter().filter(recallable).map(|n| candidate(n, MENTION_PATH_SCORE, format!("mentions `{path}`"))).collect();
    let name = path.rsplit('/').next().unwrap_or(path);
    if name != path && store.count_files_named(repo, name)? == 1 {
        let seen: HashSet<i64> = out.iter().map(|c| c.pointer.node_id).collect();
        out.extend(store.nodes_mentioning(repo, name, RECALL_KINDS, PER_SOURCE)?.into_iter().filter(|n| recallable(n) && !seen.contains(&n.id)).map(|n| candidate(n, MENTION_NAME_SCORE, format!("mentions `{name}`"))));
    }
    Ok(out)
}

/// One `SimilarTo` hop from each source-1/2 candidate, at most
/// `HOP_MAX_PER_SOURCE` neighbours each, closest first.
fn hop_candidates(store: &dyn GraphStore, repo: &str, sources: &[WorkCandidate]) -> Result<Vec<WorkCandidate>> {
    let mut out = Vec::new();
    for source in sources {
        let mut taken = 0;
        for (cosine, target) in closest_similar(store, repo, source.pointer.node_id)? {
            if taken == HOP_MAX_PER_SOURCE {
                break;
            }
            let Some(neighbour) = store.get_node(repo, target)? else { continue };
            if !recallable(&neighbour) {
                continue;
            }
            taken += 1;
            out.push(candidate(neighbour, source.score * cosine * HOP_FACTOR, format!("similar to \"{}\"", source.pointer.title)));
        }
    }
    Ok(out)
}

/// The file as a query: its name, its parent directory unless that's
/// generic, its first heading, and up to `FILE_CUE_SYMBOLS` top-level
/// symbol names, cut to `FILE_CUE_TEXT_MAX` characters.
pub fn file_query(path: &str, heading: Option<&str>, symbols: &[agentops_graph::Node]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut segments = path.rsplit('/');
    if let Some(name) = segments.next() {
        parts.push(name.to_string());
    }
    if let Some(dir) = segments.next().filter(|d| !GENERIC_DIRS.contains(d)) {
        parts.push(dir.to_string());
    }
    if let Some(heading) = heading {
        parts.push(heading.to_string());
    }
    parts.extend(symbols.iter().filter(|s| s.container.is_none()).filter_map(|s| s.name.clone()).take(FILE_CUE_SYMBOLS));
    let text = parts.join(" ");
    text.chars().take(FILE_CUE_TEXT_MAX).collect()
}

/// The first heading of a file's text: a Markdown `#` heading or a
/// leading comment (`//!`, `//`, `#`, `--`, `/*`), with its markers
/// stripped. Blank lines and a shebang are skipped; code before any comment
/// means there's no heading.
pub fn first_heading(text: &str) -> Option<String> {
    for line in text.lines().take(20) {
        let line = line.trim();
        if line.is_empty() || line.starts_with("#!") || line == "---" {
            continue;
        }
        let stripped = ["//!", "///", "//", "#", "--", "/*", "*"].iter().find_map(|m| line.strip_prefix(m))?;
        let heading = stripped.trim_start_matches('#').trim().trim_end_matches("*/").trim();
        return (!heading.is_empty()).then(|| heading.chars().take(120).collect());
    }
    None
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
    let lines: Vec<String> = text.lines().filter(|l| !l.trim_start().starts_with("Exit code")).map(clean_line).filter(|l| !l.is_empty()).collect();
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
        assert!(resolve_cue(&store, "r", &WorkCue::File { path: "target/gen.rs".into(), heading: None }, CueSources::All, None).unwrap().is_empty());
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
    fn a_doc_with_no_affects_link_gets_candidates_from_the_file_as_a_query() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        file(&store, "docs/release.md");
        let target = memory(&store, NodeKind::Gotcha, "Tagging a release needs the changelog bumped first", "Release tagging fails unless the changelog entry exists.");
        memory(&store, NodeKind::Gotcha, "Unrelated", "Webhooks retry three times.");
        let cue = WorkCue::File { path: "docs/release.md".into(), heading: Some("Release checklist: tagging and changelog".into()) };
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
    fn the_first_heading_is_a_markdown_heading_or_a_leading_comment() {
        assert_eq!(first_heading("\n# Release checklist\n\nbody").as_deref(), Some("Release checklist"));
        assert_eq!(first_heading("#!/bin/sh\n# Installs the CLI\nset -e").as_deref(), Some("Installs the CLI"));
        assert_eq!(first_heading("//! Cue resolution for recall\nuse x;").as_deref(), Some("Cue resolution for recall"));
        assert_eq!(first_heading("fn main() {}\n// later").as_deref(), None);
    }

    #[test]
    fn the_file_query_names_the_file_its_directory_heading_and_symbols() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let path = "agentops-core/crates/agentops-mcp/src/hooks.rs";
        node(&store, NodeKind::Symbol, path, "handle");
        upsert_node(&store, NewNode { kind: NodeKind::Symbol, repo: "r".into(), path: Some(path.into()), name: Some("inner".into()), container: Some("Outer".into()), start_line: None, end_line: None, content: None }).unwrap();
        let q = file_query(path, Some("Hook handlers"), &store.nodes_in_file("r", path).unwrap());
        assert_eq!(q, "hooks.rs Hook handlers handle");
        assert_eq!(file_query("deploy/compose/docker-compose.yml", None, &[]), "docker-compose.yml compose");
        assert!(file_query(&"x".repeat(400), None, &[]).chars().count() <= FILE_CUE_TEXT_MAX);
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
        let got = resolve_cue(&store, "r", &WorkCue::File { path: "src/a.rs".into(), heading: None }, CueSources::AffectsOnly, None).unwrap();
        assert_eq!(got.iter().map(|c| c.pointer.node_id).collect::<Vec<_>>(), vec![both, one]);
        assert_eq!(got[0].pointer.reason, "affects `f`, `g`");
    }
}
