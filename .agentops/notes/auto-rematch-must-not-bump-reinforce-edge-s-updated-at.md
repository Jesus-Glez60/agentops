---
title: "Auto-rematch must not bump reinforce_edge's updated_at"
type: gotcha
---

GraphStore::reinforce_edge now takes a bump_confirmed_at bool. The automatic every-scan note rematch (agentops_notes::ingest_vault called from scan::persist, Module B) must pass false, not true — it's a blind keyword rematch, not a human reconfirming a gotcha/decision is still accurate. Passing true there (the original bug, caught live-testing Phase 2's bi-temporal staleness surfacing) refreshed every Affects edge's updated_at on every rescan regardless of whether the target symbol actually changed, which permanently defeated tool_get_symbol's staleness comparison against node_history — a gotcha always looked freshly confirmed the instant a rescan ran. Only explicit human-initiated reinforcement (re-adding the same note via add_note/ingest_notes_dir) should pass true.
