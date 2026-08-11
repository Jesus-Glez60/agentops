---
title: "Module 6 session correlation is protocol-native via session_id, not a new subsystem"
type: decision
---

Cross-tool session correlation (Module 6) is implemented as an optional session_id: Option<String> arg on every write tool (scan_repo, add_note, ingest_notes, explain_symbol), recorded into a new repo-scoped session_events table (GraphStore::record_session_event/session_events, both SQLite and Postgres). Any MCP/REST client passing the same session_id across calls produces one correlated feed via the new get_session tool. No new server/protocol concept — works for any client already speaking MCP/REST, deliberately not AgentOps-specific. Directly consumed by Module 7 (task manager): a task's session_id will join into session_events for its 'final audit' activity feed.
