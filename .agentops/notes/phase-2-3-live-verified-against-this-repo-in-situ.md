---
title: "Phase 2/3 live-verified against this repo, in situ"
type: gotcha
---

Confirmed against the real AgentOps repo, not a scratch repo: the reinforce_edge gotcha (recorded earlier this session, describing the bump_confirmed_at bug fix) correctly flipped to possibly-stale after rescanning post-fix, because the symbols content changed after the gotcha edge was last confirmed. Also confirmed a task created via agentops task create --session-id, followed by an MCP scan_repo call sharing that session_id, correctly shows up in get_task_activity/agentops task activity for this real repo.
