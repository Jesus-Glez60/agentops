---
title: "notes written before first scan never attach to symbols"
type: gotcha
---

agentops note/add_note matches note bodies against symbols already in the graph at write time via WordBoundaryMatcher, then never retroactively rematches. Writing gotchas/decisions against a repo that hasn't been scanned yet (or scanning it later) leaves them permanently unattached — zero Affects edges — even if the note text names real, later-indexed symbols verbatim. Discovered live via MCP: the 5 gotchas recorded during this session's own Module I dogfooding pass had zero edges, because agentops note ran before agentops install had populated any symbols. Fixed operationally by re-running agentops ingest-notes after the repo is scanned, which re-matches and dedupes edges via connect_many — not a code fix, a workflow-ordering gotcha: always scan before (or re-ingest after) writing notes.
