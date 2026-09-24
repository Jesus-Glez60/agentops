//! Builds the session-resume guide — a prioritized, size-budgeted summary
//! of one session's state (active task, files touched, decisions, errors,
//! git ops, external references, stated intent). Shared by the
//! `get_session_guide` MCP tool (`tools.rs`) and `agentops-cli`'s
//! `SessionStart` hook handler (`hook_capture.rs`) — one implementation,
//! two callers, so the tool and the hook can never drift apart.
//!
//! Deliberately doesn't cover "Rejected Approaches"/"Constraints"/
//! "Blockers" — those need actual semantic judgment (an LLM call), not
//! pattern-matching, and are out of scope for this pass (see the plan this
//! was built from).

use agentops_graph::GraphStore;

use crate::budget::cap_sections;

/// ~2KB total, matching Context Mode's own session-resume snapshot budget —
/// the one concrete number either project publishes for this kind of
/// summary (per-tool-response caps elsewhere in this crate use a different,
/// larger budget — see `budget::DEFAULT_CHAR_BUDGET` — since a guide is
/// meant to be skimmed at a glance, not a full result set).
pub const GUIDE_CHAR_BUDGET: usize = 2000;

/// Builds the guide for `session_id` in `repo`. Returns an empty string if
/// there's nothing to show (never an error for "no activity yet" — that's
/// the normal case for a brand-new session).
pub fn build(store: &dyn GraphStore, repo: &str, session_id: &str) -> anyhow::Result<String> {
    let events = store.session_events(repo, session_id)?;
    if events.is_empty() {
        return Ok(String::new());
    }

    let task_section = store
        .list_tasks(repo)?
        .into_iter()
        .find(|t| t.session_id.as_deref() == Some(session_id))
        .map(|t| format!("{} ({})", t.title, t.status.as_db_str()))
        .unwrap_or_default();

    let last_n = |kind: &str, n: usize| -> String {
        let matched: Vec<&str> = events.iter().filter(|e| e.event_kind == kind).map(|e| e.description.as_str()).collect();
        let start = matched.len().saturating_sub(n);
        matched[start..].iter().map(|d| format!("- {d}")).collect::<Vec<_>>().join("\n")
    };

    let sections: Vec<(&str, String)> = vec![
        ("Task", task_section),
        ("Session intent", last_n("user_intent", 1)),
        ("Files modified", last_n("file_modified", 10)),
        ("Decisions", last_n("decision", 5)),
        ("Unresolved errors", last_n("error", 5)),
        ("Git ops", last_n("git_op", 5)),
        ("External references", last_n("external_reference", 5)),
        ("Secret-exposure warnings", last_n("secret_exposure_warning", 3)),
    ];

    Ok(cap_sections(&sections, GUIDE_CHAR_BUDGET))
}

#[cfg(test)]
mod tests {
    use agentops_graph::SqliteGraphStore;

    use super::*;

    #[test]
    fn empty_session_produces_an_empty_guide() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let guide = build(&store, "demo", "never-used-session").unwrap();
        assert_eq!(guide, "");
    }

    #[test]
    fn a_session_with_activity_produces_a_multi_section_guide() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.record_session_event("demo", "sess-1", "user_prompt", "fix the login bug", None, "user_intent").unwrap();
        store.record_session_event("demo", "sess-1", "Edit", "src/auth.rs", None, "file_modified").unwrap();
        store.record_session_event("demo", "sess-1", "Bash", "git commit -m 'fix login'", None, "git_op").unwrap();

        let guide = build(&store, "demo", "sess-1").unwrap();
        assert!(guide.contains("## Session intent"));
        assert!(guide.contains("fix the login bug"));
        assert!(guide.contains("## Files modified"));
        assert!(guide.contains("src/auth.rs"));
        assert!(guide.contains("## Git ops"));
    }

    #[test]
    fn only_events_for_the_requested_session_are_included() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.record_session_event("demo", "sess-a", "Edit", "a.rs", None, "file_modified").unwrap();
        store.record_session_event("demo", "sess-b", "Edit", "b.rs", None, "file_modified").unwrap();

        let guide = build(&store, "demo", "sess-a").unwrap();
        assert!(guide.contains("a.rs"));
        assert!(!guide.contains("b.rs"));
    }

    #[test]
    fn includes_the_active_task_for_this_session() {
        use agentops_graph::{NewTask, TaskStatus};

        let store = SqliteGraphStore::open_in_memory().unwrap();
        store.create_task(NewTask { repo: "demo".into(), title: "Fix the login bug".into(), description: None, status: TaskStatus::InProgress, priority: None, assignee: None, external_source: None, external_id: None, session_id: Some("sess-1".into()) }).unwrap();
        store.record_session_event("demo", "sess-1", "Edit", "src/auth.rs", None, "file_modified").unwrap();

        let guide = build(&store, "demo", "sess-1").unwrap();
        assert!(guide.contains("## Task"));
        assert!(guide.contains("Fix the login bug"));
        assert!(guide.contains("in_progress"));
    }
}
