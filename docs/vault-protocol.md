# Notes vault protocol

The protocol for recording a gotcha, decision, or knowledge note into a
project's AgentOps knowledge graph — referenced by the `save-note` and
`wrap` skills and the `vault-archivist` subagent. Applies to any repo,
including this one (AgentOps dogfoods itself).

## Step 1: check whether this repo is remote-connected

Before writing anything, check whether `.context/agentops-remote.json`
exists in the repo root (the same marker `agentops connect --remote` and
`agentops usage sync --remote` write — see
`agentops-core/crates/agentops-cli/src/main.rs`'s `remote_marker_path`).

- **If it exists**: this repo's real graph lives on a remote,
  Postgres-backed server. The `add_note`/`ingest_notes` MCP tool (or
  `POST /tools/add_note` over REST with an API key) is always reachable
  and is the *only* correct way to record a note. **Never write a local
  Markdown file in this case** — if the agent doing the recording doesn't
  have the MCP tool available, that's a reason to escalate to one that
  does (or call the tool directly instead of delegating), not a reason to
  fall back to a file write. A local file write here just produces an
  orphaned note nobody's dashboard or `list_gotchas` call will ever see.
- **If it's absent**: there is no remote connection at all. This is the
  only case where the local-file fallback below applies.

This check is mechanical and cheap (one `Read` of one file) — there's no
judgment call about "is the MCP tool available," which is exactly the
ambiguity that has repeatedly caused notes to land as orphaned local files
in remote-connected repos (see this repo's own recorded gotcha/feedback
history on the subject).

## Step 2: record the note

**Remote-connected (preferred, and mandatory when the marker exists):**
call `add_note` (single note) or `ingest_notes` (a whole folder at once)
over MCP, or the equivalent REST route. Required fields: `path` (the repo
connection id), `title`, `body`. Optional: `note_type`
(`gotcha`/`decision`/`knowledge` — omit to auto-classify),
`tags`, `with_embeddings` (set true to make it semantic-search-findable),
`session_id` (correlates the write into a session's activity feed).

**Local fallback (only when no remote marker exists):** write a Markdown
file under `NOTES_PATH` (from `AGENTS.md`) with this frontmatter:

```markdown
---
title: Short, specific title
type: gotcha | decision | knowledge
tags: [relevant, keywords]
status: active
created: 2026-01-01
---

Body text — what happened, why it matters, how to apply it. Prose, not a
code block (this becomes the note body an agent reads later).
```

## Step 3: confirm

State the exact path or note id written, and which path was used
(`add_note`/`ingest_notes` vs. a direct file write) — this is what lets a
reviewer (human or another agent) catch a wrongly-local write immediately
instead of discovering it days later when the dashboard doesn't show it.

## Session end (referenced by the `wrap` skill)

At the end of a session or a substantial piece of work, sweep for anything
worth recording that wasn't already written incrementally as it surfaced:

1. Review the session's actual changes (`git status`/`git diff`), not just
   memory of what happened — a finding is worth a note if it changed a
   decision, contradicted an assumption, or would have caused a real bug
   had it not been caught.
2. For each finding, run Steps 1–3 above — check the remote marker first,
   every time, even if earlier notes in the same sweep already established
   the repo is remote-connected (cheap to re-check, and it removes any
   chance of the check being skipped for a later note in the batch).
3. Delegating this sweep to a subagent (e.g. `vault-archivist`) is fine,
   but the subagent must actually have the `add_note`/`ingest_notes` MCP
   tool available in its own tool list — granting it only `Read`/`Bash`
   and expecting it to "prefer" a tool it cannot call is what caused this
   exact bug previously.
