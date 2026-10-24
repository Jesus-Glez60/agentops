//! `recall-stats` (milestone 4 item 2): per source and memory class, how
//! many memories recall showed and how many of them the agent then looked
//! up (`memory_followed`), the one use signal milestone 3c found that
//! separates useful pointers from noise. Counts are distinct
//! `(session, context, memory)` triples, so a memory followed twice in one
//! context counts once. Eval sessions (`recall-eval-*`) are left out
//! (gotcha 112998). Pins aren't claimed when shown, so their row has
//! follows but no shown count.

use std::collections::{BTreeMap, HashMap, HashSet};

use agentops_graph::GraphStore;
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::hooks::MEMORY_FOLLOWED;

/// Session ids `recall-eval` and `work-recall-eval` record under.
const EVAL_SESSION_PREFIX: &str = "recall-eval-";

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
    let rows = cells.into_iter().map(|((source, memory_class), (s, f))| StatsRow { shown: (source != "pin").then_some(s.len()), followed: f.len(), source, memory_class }).collect::<Vec<_>>();
    Ok(RecallStats {
        sessions_shown: shown.iter().map(|r| &r.session_id).collect::<HashSet<_>>().len(),
        sessions_followed: follow_sessions.len(),
        total_followed: rows.iter().map(|r| r.followed).sum(),
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
    }
}
