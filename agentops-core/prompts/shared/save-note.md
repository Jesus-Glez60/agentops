Save a project note for the current finding.

Read `AGENTS.md` and extract `NOTES_PATH`. If it's missing, stop and ask —
do not guess a path.

Check whether `.context/agentops-remote.json` exists in the repo. If it
does, this repo is connected to a real AgentOps server and `add_note`/
`ingest_notes` is always reachable — always use it, never a direct file
write, even when delegating. Only when that marker is absent does the
direct-file fallback below apply.

If a `vault-archivist` subagent is available, delegate the save to it with
the title/body/type inferred from the request above (it performs the same
remote-marker check itself). Otherwise call `add_note`/`ingest_notes`
directly, or — only if the remote marker is absent — write a frontmattered
Markdown file under `NOTES_PATH`:

```yaml
---
title: "[Descriptive title]"
type: gotcha|decision|knowledge
tags: []
status: active
created: YYYY-MM-DD
---
```

Confirm with the exact path/note id written and how (`add_note`/
`ingest_notes` vs. direct file write).
