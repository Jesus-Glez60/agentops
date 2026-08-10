# NOTES WRITE-BACK PROTOCOL

## ABSOLUTE RULES — Never deviate

1. **ALWAYS read `AGENTS.md`** before writing a note. Extract the exact `NOTES_PATH`.
2. **NEVER write to a path that differs from `NOTES_PATH`** in `AGENTS.md`.
3. **Prefer the `add_note` MCP tool over a direct file write.** It validates,
   classifies (if `note_type` is omitted), and ingests atomically — a
   hand-typed frontmatter file can't guarantee any of that.
4. **Only write the file directly if no MCP connection to this project's
   AgentOps server is available.** In that case the file still needs valid
   YAML frontmatter (see template below) and must live under `NOTES_PATH`.
5. **If `AGENTS.md` is missing or has no `NOTES_PATH`**: STOP. Ask the user
   for the path, or offer to run the AGENTS.md generator. Do not guess a
   path, and never fall back to a path from a different project or a prior
   session.
6. **NEVER invent slugs** — derive them from the note title in
   lowercase-hyphenated form.

---

## Preferred path: `add_note`

Call the `add_note` MCP tool (or `POST /tools/add_note` over REST if no MCP
connection is available) with:

```json
{
  "title": "Descriptive title",
  "body": "The note content, in Markdown.",
  "note_type": "gotcha | decision | note (optional — omit to let the classifier decide)",
  "tags": ["optional", "tags"]
}
```

This writes a correctly-frontmattered file under `NOTES_PATH` *and* ingests
it into the graph in one step. Confirm with the path it reports back.

---

## Fallback path: direct file write

Only used when no MCP/REST connection to the project's AgentOps server is
reachable. Write a new Markdown file under `NOTES_PATH` with this
frontmatter:

```yaml
---
title: "[Descriptive title]"
type: gotcha|decision|note|knowledge
tags: []
status: active
created: YYYY-MM-DD
---
```

The body follows the frontmatter. This file will be picked up the next time
notes are ingested (manually, or automatically on the next `scan_repo`/
ingestion pass) — it is not immediately queryable the way `add_note` is.

---

## After Writing

Always confirm:
> "Saved note: `[exact path]` (via add_note | direct file write)"
