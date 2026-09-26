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

use agentops_graph::{GraphStore, NodeKind, NodeProminence};

use crate::budget::cap_sections;

/// ~2KB total, matching Context Mode's own session-resume snapshot budget —
/// the one concrete number either project publishes for this kind of
/// summary (per-tool-response caps elsewhere in this crate use a different,
/// larger budget — see `budget::DEFAULT_CHAR_BUDGET` — since a guide is
/// meant to be skimmed at a glance, not a full result set).
pub const GUIDE_CHAR_BUDGET: usize = 2000;

/// Sort key for a 3-way `NodeProminence` ordering (lower sorts first):
/// `Pinned` (a human's standing directive) ahead of `Full`, `Reduced` last.
/// Replaces an earlier 2-variant `prominence == Reduced` boolean trick that
/// no longer expresses a 3-way order now that `Pinned` exists.
fn prominence_sort_rank(p: NodeProminence) -> u8 {
    match p {
        NodeProminence::Pinned => 0,
        NodeProminence::Full => 1,
        NodeProminence::Reduced => 2,
    }
}

/// Sticky-pin context: nodes a human has explicitly pinned
/// (`NodeProminence::Pinned`), meant to be injected on every `SessionStart`
/// *and* every `UserPromptSubmit` -- unlike `build`/`build_repo_briefing`
/// (session-activity/scan-derived, only meaningful at resume/compact), a
/// pin is a standing directive the agent should never lose sight of
/// mid-session, so this is called from both hook handlers
/// (`agentops-cli::hook_capture`). Returns an empty string when there are
/// no pins -- same "nothing to show is the normal case" contract as `build`.
pub fn build_pinned_context(store: &dyn GraphStore, repo: &str) -> anyhow::Result<String> {
    let mut pinned = store.nodes_pinned(repo)?;
    if pinned.is_empty() {
        return Ok(String::new());
    }
    // Stable order across calls regardless of underlying query order, so
    // the injected block doesn't reshuffle every turn.
    pinned.sort_by_key(|n| n.id);

    let body = pinned
        .iter()
        .map(|n| {
            let name = n.name.as_deref().unwrap_or("(untitled)");
            let excerpt = n.content.as_deref().unwrap_or("").lines().next().unwrap_or("");
            format!("- [{}] {name}: {excerpt} (id {})", n.kind.as_db_str(), n.id)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Name+excerpt only, not full content -- a caller wanting the full body
    // already has `fetch_content` by node id (same excerpt-only pattern as
    // `build_repo_briefing`'s `excerpt_section`, not `process_post_tool_use`'s
    // full-capture pattern).
    let sections: [(&str, String); 1] = [("Pinned", body)];
    Ok(cap_sections(&sections, GUIDE_CHAR_BUDGET))
}

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

/// Repo-level fallback for a session with no activity yet (a brand-new
/// session, or `SessionStart` firing on `source: "startup"`) — `build`
/// alone has nothing to show at that point, but the repo itself usually
/// already has scan history and recorded gotchas/decisions worth surfacing
/// immediately, without the agent spending its own tokens on separate
/// `status`/`list_gotchas` calls to get the same picture.
///
/// Reuses `list_gotchas`' live full-prominence-first sort (`tools.rs`'s
/// `tool_list_gotchas`), not `status`'s precomputed `repo_state.top_gotcha_ids`
/// — simpler, and doesn't depend on `repo_state` having been populated
/// (checked both existing patterns before picking one, they differ).
pub fn build_repo_briefing(store: &dyn GraphStore, repo: &str) -> anyhow::Result<String> {
    let scan_section = match store.latest_scan(repo)? {
        Some(scan) => format!("{repo} — last scan {}: files +{} ~{} -{}, symbols +{} ~{} -{}", scan.started_at, scan.files_added, scan.files_changed, scan.files_removed, scan.symbols_added, scan.symbols_changed, scan.symbols_removed),
        None => String::new(),
    };
    // Never scanned — nothing else here would have any content either.
    if scan_section.is_empty() {
        return Ok(String::new());
    }

    let excerpt_section = |kind: NodeKind| -> anyhow::Result<String> {
        let mut nodes = store.nodes_by_kind(repo, kind)?;
        nodes.sort_by_key(|n| prominence_sort_rank(n.prominence));
        Ok(nodes
            .iter()
            .take(5)
            .map(|n| {
                let name = n.name.as_deref().unwrap_or("(untitled)");
                let excerpt = n.content.as_deref().unwrap_or("").lines().next().unwrap_or("");
                format!("- {name}: {excerpt}")
            })
            .collect::<Vec<_>>()
            .join("\n"))
    };

    let sections: Vec<(&str, String)> = vec![
        ("Repo", scan_section),
        ("Top gotchas", excerpt_section(NodeKind::Gotcha)?),
        ("Top decisions", excerpt_section(NodeKind::Decision)?),
        ("More knowledge", "See AGENTS.md and this repo's notes (list_gotchas/related_context/get_symbol) for the full picture — this is a short briefing, not the whole graph.".to_string()),
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

    #[test]
    fn pinned_context_is_empty_with_no_pins() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let ctx = build_pinned_context(&store, "demo").unwrap();
        assert_eq!(ctx, "");
    }

    #[test]
    fn pinned_context_surfaces_a_pinned_gotcha_but_not_an_unpinned_one() {
        use agentops_graph::NewNode;

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let pinned_id = store.add_node(NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("Always remember".into()), container: None, start_line: None, end_line: None, content: Some("Never bypass the redaction gate.".into()) }).unwrap();
        store.add_node(NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("Unrelated".into()), container: None, start_line: None, end_line: None, content: Some("Some other note.".into()) }).unwrap();
        store.set_curation("demo", pinned_id, NodeProminence::Pinned, None).unwrap();

        let ctx = build_pinned_context(&store, "demo").unwrap();
        assert!(ctx.contains("## Pinned"), "{ctx}");
        assert!(ctx.contains("Always remember"), "{ctx}");
        assert!(ctx.contains("Never bypass the redaction gate."), "{ctx}");
        assert!(!ctx.contains("Unrelated"), "{ctx}");
    }

    #[test]
    fn repo_briefing_is_empty_for_a_never_scanned_repo() {
        let store = SqliteGraphStore::open_in_memory().unwrap();
        let briefing = build_repo_briefing(&store, "demo").unwrap();
        assert_eq!(briefing, "");
    }

    #[test]
    fn repo_briefing_surfaces_scan_stats_and_gotchas_for_a_fresh_session() {
        use agentops_graph::{NewNode, NewScanHistoryEntry, ScanChange};

        let store = SqliteGraphStore::open_in_memory().unwrap();
        let symbol_id = store.add_node(NewNode { kind: NodeKind::Symbol, repo: "demo".into(), path: Some("src/auth.rs".into()), name: Some("login".into()), container: None, start_line: None, end_line: None, content: Some("fn login() {}".into()) }).unwrap();
        store.record_scan("demo", &[NewScanHistoryEntry { node_id: symbol_id, kind: NodeKind::Symbol, path: Some("src/auth.rs".into()), name: Some("login".into()), change: ScanChange::Added }]).unwrap();
        store.add_node(NewNode { kind: NodeKind::Gotcha, repo: "demo".into(), path: None, name: Some("Watch out".into()), container: None, start_line: None, end_line: None, content: Some("Edge case in the login flow.".into()) }).unwrap();

        let briefing = build_repo_briefing(&store, "demo").unwrap();
        assert!(briefing.contains("## Repo"), "{briefing}");
        assert!(briefing.contains("symbols +1"), "{briefing}");
        assert!(briefing.contains("## Top gotchas"), "{briefing}");
        assert!(briefing.contains("Watch out"), "{briefing}");
    }
}
