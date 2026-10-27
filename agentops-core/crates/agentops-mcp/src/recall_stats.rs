//! `recall-stats` (milestone 4 item 2): per source and memory class, how
//! many memories recall showed and how many of them the agent then looked
//! up (`memory_followed`), the one use signal milestone 3c found that
//! separates useful pointers from noise. Counts are distinct
//! `(session, context, memory)` triples, so a memory followed twice in one
//! context counts once. Eval sessions (`recall-eval-*`) are left out
//! (gotcha 112998). Pins aren't claimed when shown, so their row has
//! follows but no shown count.
//!
//! Sessions are also split into **meta** and **ordinary** (milestone 5
//! item 7): a meta session read or edited AgentOps' own memory and recall
//! code (`META_PATHS`), so what it followed says little about ordinary
//! work. Paths come from `file_read`/`file_modified` events and work-time
//! file cues, which only some sessions record (gotcha 195365), so meta is
//! undercounted, never overcounted.

use std::collections::{BTreeMap, HashMap, HashSet};

use agentops_graph::GraphStore;
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::hooks::MEMORY_FOLLOWED;

/// Session ids `recall-eval` and `work-recall-eval` record under.
const EVAL_SESSION_PREFIX: &str = "recall-eval-";

/// Path fragments of AgentOps' memory and recall code. A session that read
/// or edited any of them is meta. They only occur in the AgentOps repo.
pub const META_PATHS: &[&str] = &[
    "agentops-mcp/src/work_recall.rs",
    "agentops-mcp/src/hooks.rs",
    "agentops-mcp/src/proposals.rs",
    "agentops-mcp/src/session_guide.rs",
    "agentops-mcp/src/budget.rs",
    "agentops-mcp/src/recall_stats.rs",
    "agentops-retrieval/",
    "agentops-graph/",
    "agentops-cli/src/recall_eval.rs",
    "agentops-cli/src/work_recall_eval.rs",
    "agentops-cli/src/import_memory.rs",
];

/// One source × class row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsRow {
    pub source: String,
    pub memory_class: String,
    /// `None` for pins, which aren't recorded when shown.
    pub shown: Option<usize>,
    pub followed: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallStats {
    pub rows: Vec<StatsRow>,
    /// Sessions with at least one shown memory, and with at least one follow.
    pub sessions_shown: usize,
    pub sessions_followed: usize,
    pub total_followed: usize,
    /// Of the sessions that were shown or followed a memory, how many were
    /// meta and how many ordinary.
    #[serde(default)]
    pub sessions_meta: usize,
    #[serde(default)]
    pub sessions_ordinary: usize,
    /// Follows in ordinary sessions only, per source.
    #[serde(default)]
    pub ordinary_followed: BTreeMap<String, usize>,
}

/// The sessions of `repo` that touched `META_PATHS`.
pub fn meta_sessions(store: &dyn GraphStore, repo: &str) -> Result<HashSet<String>> {
    let mut meta = HashSet::new();
    for kind in [crate::hooks::FILE_READ, "file_modified"] {
        for e in store.session_events_for_repo(repo, Some(kind))? {
            if META_PATHS.iter().any(|p| e.description.contains(p)) {
                meta.insert(e.session_id);
            }
        }
    }
    for e in store.session_events_for_repo(repo, Some(crate::work_recall::WORK_CUE))? {
        if META_PATHS.iter().any(|p| e.tool_name.contains(p)) {
            meta.insert(e.session_id);
        }
    }
    Ok(meta)
}

/// The stats for `repo`, counting only what was shown or followed at or
/// after `since` (a date or timestamp) when given.
pub fn recall_stats(store: &dyn GraphStore, repo: &str, since: Option<&str>) -> Result<RecallStats> {
    let real = |session: &str| !session.starts_with(EVAL_SESSION_PREFIX);
    let shown: Vec<_> = store.recall_shown_rows(repo, None, since)?.into_iter().filter(|r| real(&r.session_id)).collect();
    let followed: Vec<_> = store.session_events_for_repo(repo, Some(MEMORY_FOLLOWED))?.into_iter().filter(|e| real(&e.session_id) && since.is_none_or(|s| e.created_at.as_str() >= s)).collect();

    let mut classes: HashMap<i64, String> = HashMap::new();
    let mut class_of = |id: i64| -> Result<String> {
        if let Some(c) = classes.get(&id) {
            return Ok(c.clone());
        }
        let c = store.get_node(repo, id)?.map(|n| n.memory_class.as_db_str().to_string()).unwrap_or_else(|| "(gone)".to_string());
        classes.insert(id, c.clone());
        Ok(c)
    };

    // (source, class) -> (shown triples, followed triples)
    type Triple = (String, String, i64);
    let mut cells: BTreeMap<(String, String), (HashSet<Triple>, HashSet<Triple>)> = BTreeMap::new();
    for r in &shown {
        let key = (r.source.clone(), class_of(r.node_id)?);
        cells.entry(key).or_default().0.insert((r.session_id.clone(), r.context.clone(), r.node_id));
    }
    let mut follow_sessions = HashSet::new();
    for e in &followed {
        let Some(id) = e.node_id else { continue };
        // `id N source S context C`
        let field = |name: &str| e.description.split(' ').skip_while(|w| *w != name).nth(1).unwrap_or("unknown").to_string();
        let (source, context) = (field("source"), field("context"));
        cells.entry((source, class_of(id)?)).or_default().1.insert((e.session_id.clone(), context, id));
        follow_sessions.insert(e.session_id.clone());
    }
    let meta = meta_sessions(store, repo)?;
    let mut ordinary_followed: BTreeMap<String, usize> = BTreeMap::new();
    for ((source, _), (_, f)) in &cells {
        let n = f.iter().filter(|(session, _, _)| !meta.contains(session)).count();
        if n > 0 {
            *ordinary_followed.entry(source.clone()).or_default() += n;
        }
    }
    let rows = cells.into_iter().map(|((source, memory_class), (s, f))| StatsRow { shown: (source != "pin").then_some(s.len()), followed: f.len(), source, memory_class }).collect::<Vec<_>>();
    let sessions: HashSet<&String> = shown.iter().map(|r| &r.session_id).chain(&follow_sessions).collect();
    let sessions_meta = sessions.iter().filter(|s| meta.contains(s.as_str())).count();
    Ok(RecallStats {
        sessions_shown: shown.iter().map(|r| &r.session_id).collect::<HashSet<_>>().len(),
        sessions_followed: follow_sessions.len(),
        total_followed: rows.iter().map(|r| r.followed).sum(),
        sessions_meta,
        sessions_ordinary: sessions.len() - sessions_meta,
        ordinary_followed,
        rows,
    })
}

/// The stats as a table, plus the session counts.
pub fn render(stats: &RecallStats) -> String {
    let mut out = String::from("| Source | Class | Shown | Followed | Rate |\n| --- | --- | --- | --- | --- |\n");
    for r in &stats.rows {
        let (shown, rate) = match r.shown {
            Some(0) | None => (r.shown.map_or("-".to_string(), |n| n.to_string()), "-".to_string()),
            Some(n) => (n.to_string(), format!("{:.1}%", 100.0 * r.followed as f64 / n as f64)),
        };
        out += &format!("| {} | {} | {shown} | {} | {rate} |\n", r.source, r.memory_class, r.followed);
    }
    out += &format!("\n{} follows; {} sessions shown memories, {} followed one.\n", stats.total_followed, stats.sessions_shown, stats.sessions_followed);
    let ordinary: usize = stats.ordinary_followed.values().sum();
    let by_source = stats.ordinary_followed.iter().map(|(s, n)| format!("{s} {n}")).collect::<Vec<_>>().join(", ");
    out += &format!("Sessions: {} ordinary, {} meta (touched AgentOps' memory or recall code). Ordinary follows: {ordinary}{}.\n", stats.sessions_ordinary, stats.sessions_meta, if by_source.is_empty() { String::new() } else { format!(" ({by_source})") });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{upsert_node, MemoryClass, NewNode, NodeKind, SqliteGraphStore};

    fn memory(store: &SqliteGraphStore, name: &str) -> i64 {
        upsert_node(store, NewNode { kind: NodeKind::Gotcha, repo: "r".into(), path: Some(format!("vault:{name}")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(name.into()) }).unwrap()
    }

    fn follow(store: &SqliteGraphStore, session: &str, id: i64, source: &str, agent: Option<&str>) {
        let context = agent.unwrap_or("main#0");
        store.record_session_event_in("r", session, "mcp__agentops__fetch_content", &format!("id {id} source {source} context {context}"), Some(id), MEMORY_FOLLOWED, agent).unwrap();
    }

    #[test]
    fn counts_distinct_shown_and_followed_per_source_and_class_without_eval_sessions() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let (a, b, c) = (memory(&store, "a"), memory(&store, "b"), memory(&store, "c"));
        store.set_memory_class("r", c, MemoryClass::Experimental, None).unwrap();
        store.claim_shown("r", "s1", "main#0", &[a, b, c], "prompt_seed").unwrap();
        store.claim_shown("r", "s1", "main#0", &[a], "file_cue").unwrap(); // already shown: no row
        store.claim_shown("r", "s2", "a1", &[a], "subagent_seed").unwrap();
        store.claim_shown("r", "recall-eval-1", "main#0", &[a, b], "prompt_seed").unwrap();
        follow(&store, "s1", a, "prompt_seed", None);
        follow(&store, "s1", a, "prompt_seed", None); // twice in one context counts once
        follow(&store, "s1", c, "prompt_seed", None);
        follow(&store, "s2", a, "subagent_seed", Some("a1"));
        follow(&store, "s2", b, "pin", None);
        follow(&store, "recall-eval-1", a, "prompt_seed", None);

        let stats = recall_stats(&store, "r", None).unwrap();
        let row = |source: &str, class: &str| stats.rows.iter().find(|r| r.source == source && r.memory_class == class).map(|r| (r.shown, r.followed));
        assert_eq!(row("prompt_seed", "durable"), Some((Some(2), 1)));
        assert_eq!(row("prompt_seed", "experimental"), Some((Some(1), 1)));
        assert_eq!(row("subagent_seed", "durable"), Some((Some(1), 1)));
        assert_eq!(row("pin", "durable"), Some((None, 1)), "pins have follows only");
        assert_eq!((stats.total_followed, stats.sessions_shown, stats.sessions_followed), (4, 2, 2));
        assert!(render(&stats).contains("| prompt_seed | durable | 2 | 1 | 50.0% |"), "{}", render(&stats));
        assert!(recall_stats(&store, "r", Some("9999-01-01")).unwrap().rows.is_empty());
        assert_eq!((stats.sessions_meta, stats.sessions_ordinary), (0, 2), "eval sessions are in neither");
    }

    #[test]
    fn a_session_that_touched_recall_code_is_meta_and_its_follows_are_kept_out_of_the_ordinary_count() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let a = memory(&store, "a");
        for s in ["edits-recall", "reads-retrieval", "cue-on-hooks", "other-code", "plain"] {
            store.claim_shown("r", s, "main#0", &[a], "prompt_seed").unwrap();
            follow(&store, s, a, "prompt_seed", None);
        }
        store.record_session_event("r", "edits-recall", "Edit", "/Users/x/agentops/agentops-core/crates/agentops-mcp/src/work_recall.rs", None, "file_modified").unwrap();
        store.record_session_event("r", "reads-retrieval", "Read", "agentops-core/crates/agentops-retrieval/src/recall.rs", None, crate::hooks::FILE_READ).unwrap();
        store.record_session_event("r", "cue-on-hooks", "file:agentops-core/crates/agentops-mcp/src/hooks.rs", "1:0.9", None, crate::work_recall::WORK_CUE).unwrap();
        store.record_session_event("r", "other-code", "Edit", "/Users/x/agentops/apps/web/src/page.tsx", None, "file_modified").unwrap();
        let stats = recall_stats(&store, "r", None).unwrap();
        assert_eq!((stats.sessions_meta, stats.sessions_ordinary), (3, 2));
        assert_eq!(stats.ordinary_followed.get("prompt_seed"), Some(&2));
        assert_eq!(stats.total_followed, 5, "the per-source rows still count every session");
        assert!(render(&stats).contains("Sessions: 2 ordinary, 3 meta") && render(&stats).contains("Ordinary follows: 2 (prompt_seed 2)"), "{}", render(&stats));
    }
}
