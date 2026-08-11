---
title: "Module 7 task manager and Linear sync: what shipped and what's unverified"
type: decision
---

Native task manager (tasks/task_links tables, create_task/list_tasks/update_task_status/get_task_activity MCP tools, agentops task CLI subcommands) is fully implemented and live-verified against this real repo: a task's session_id correctly pulls in scan_repo/add_note activity via get_task_activity, matching the 'final audit' workflow the module was designed around. Linear sync (new agentops-linear crate, GraphQL via ureq, poll-based pull + team-workflow-state-aware push) is implemented and tested against wiremock (pull-then-push round trip without duplication, idempotent upsert_external_task preserving created_at). It has NOT been live-verified against a real Linear account/API key — none was available in this environment. Before considering Module 7 fully done, run a real pull against a live Linear workspace and confirm a local status change actually appears on the Linear issue (the exact verification the module's design doc calls for).
