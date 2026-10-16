# Spike: Claude Code auto-memory for milestone 2

Item 0 of the milestone 2 spec (MEMORY.md import). Run on 2026-10-16 against Claude Code **2.1.286**, in this repo. No code from the spike is merged. The probe was one setting added to `.claude/settings.local.json` and restored from a backup afterwards. The live check was a fresh session, `acb4b87b`. The docs answers come from `code.claude.com/docs/en/memory`, scraped into the docs graph as `claude-code-memory`. This note reports counts and field names only, no memory entry text.

## Answers

| # | Question | Answer | Evidence |
|---|---|---|---|
| 1 | What is the format of an auto-memory entry file, and how does MEMORY.md index it? | **One Markdown file per memory with YAML frontmatter. MEMORY.md is a one-line-per-entry index.** The docs give a `type` field: `user`, `feedback`, `project` or `reference`. In this repo all 49 entries nest it as `metadata.type`, beside `metadata.node_type` (48), `metadata.originSessionId` (48) and `metadata.modified` (15). Every entry also has top-level `name` and `description`. The docs say Claude Code adds `modified` when it writes a file that has frontmatter (v2.1.214+). MEMORY.md lines are `- [Title](file.md) — hook`. Only the first 200 lines or 25 KB of MEMORY.md load at startup; topic files load on demand. Subagents never get auto memory. | Docs ("Auto memory", "How it works"); a frontmatter key count over the 49 files |
| 2 | Which switch turns auto-memory off for one project? | **`autoMemoryEnabled: false` in `.claude/settings.local.json`. Verified.** The docs also list `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` (not exercised) and the `/memory` toggle, which writes the user-level setting. | The session's `prompt_snapshot` attachment. Session `a8a2c120` (before the switch): 7,502-char system prompt with a `# Memory` section and 2 `MEMORY.md` mentions. Session `acb4b87b` (switch on): 5,402 chars, with 0 `# Memory`, 0 "persistent file-based memory" and 0 `MEMORY.md`. Asked to quote its MEMORY.md index, the agent said none was loaded. |
| 3 | Does the switch also stop the writes milestone 1's redirect blocks? | **Yes in effect:** with the switch on, the agent gets no memory instructions and no memory directory path, so it has no reason to write there. Asked to "remember" a test token, it offered AgentOps instead and wrote nothing to the memory directory. **Caveat:** that session was in plan mode, which blocks writes anyway, so this rests on the missing instructions rather than on a refused write. The redirect (`hooks.rs` `is_memory_write`) stays on as the backstop, as the spec asks. | The memory directory still has 50 files, and the token is in none of them. Session `acb4b87b` transcript: no `Write`/`Edit` to `memory/`. |

## Consequences for the plan

- **Item 5 uses the per-project switch.** Set `autoMemoryEnabled: false` in `.claude/settings.local.json` (uncommitted) after the import. No user-level change is needed.
- **The importer reads `metadata.type` first, then a top-level `type`, then the file-name prefix.** On this repo the prefix always agrees with `metadata.type`.
- **The entry files are the source of truth, not MEMORY.md.** The directory has 49 entry files but MEMORY.md indexes 48 (`project_repositories_screen_roadmap.md` is missing), so the import report flags unindexed files.

## Found along the way

- **The plan-mode gate treats every plan-mode request as a full task**, including "remember this token". Session `acb4b87b` hit the evidence gate (`2ecbf3c`) on a one-line plan. That's working as designed (the requirement says entering plan mode means there is work to do), but it's a cost worth knowing.
