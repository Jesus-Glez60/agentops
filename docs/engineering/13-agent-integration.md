# Agent integration

How AgentOps plugs into coding tools: registering the MCP server, distributing instructions and skills, installing Claude Code hooks, and tracking usage. Read this before supporting a new editor or agent, or changing what `agentops connect` writes.

Tool contracts are in [07-mcp-and-rest-contracts.md](07-mcp-and-rest-contracts.md); the device-flow login is in [09-auth-teams-security.md](09-auth-teams-security.md).

## Supported tools

`agentops connect` offers five tools by name (`select_agents` in `agentops-core/crates/agentops-cli/src/main.rs`):

| Agent id | Tool | User-level MCP config | Default in the picker |
| --- | --- | --- | --- |
| `claude` | Claude Code | `~/.claude.json` | yes |
| `cursor` | Cursor | `~/.cursor/mcp.json` | yes |
| `codex` | Codex CLI | `~/.codex/config.toml` | no |
| `gemini-cli` | Gemini CLI | `~/.gemini/settings.json` | no |
| `windsurf` | Windsurf | `~/.codeium/windsurf/mcp_config.json` | no |

The interactive picker also accepts any other agent id Ruler supports. For those, AgentOps distributes instructions but prints that it cannot register the MCP server, and the user must do it by hand.

Cline, Zed and VS Code Copilot are deliberately not first-class. Decision "Cline/Zed/VS Code Copilot deliberately excluded from connect's first-class adapter list -- Windsurf added instead" gives one reason per tool:

- Cline's config sits inside VS Code's per-OS extension storage under a publisher-specific path that could change.
- Zed uses a different key (`context_servers`) and entry shape, which the shared JSON writer cannot produce.
- Copilot's docs do not fix one user-level path; it varies by OS and by VS Code edition.

Known per-tool limits, from gotchas:

- Claude Code does not load a newly registered MCP server in the current session; restart it (gotcha 54121).
- Cursor's settings UI ignores a project-level `.cursor/mcp.json` (gotcha 53985).
- Gemini CLI's `${AGENTOPS_API_KEY}` substitution in remote headers was never verified (gotcha 54000).

## connect

`agentops connect` has three modes:

| Mode | Flags | MCP entry written | Repo touched |
| --- | --- | --- | --- |
| Local | default | stdio `agentops-mcp-server` with `AGENTOPS_ACCESS_MODE` | yes |
| Remote | `--remote <url>` | Streamable HTTP entry for `<url>/mcp` with a `Bearer` personal key | yes |
| Global | `--global` (`-g`) | as above, user-level only; skills installed user-wide | no; register repos later with `register_repo` |

Other flags:

- `--agents`: comma list; skips the picker.
- `--access-mode advisor|full`: local mode only.
- `--yes`: no prompts; needs `--agents`.
- `--api-key` or `--device-login`: remote mode.
- `--no-hooks`: skip the Claude Code hooks, which are installed by default. `--with-hooks` is still accepted and does nothing.
- `--compress-commands`: command compression on the `PreToolUse` hook.

### Local mode

1. Check that `npx` and `agentops-mcp-server` are on `PATH`.
2. If the repo is not in the scan manifest, offer to scan it (`install` without Ruler).
3. Read `AGENTS.md`, or generate it with `init_agents_md` if missing.
4. Distribute instructions, skills, agents and the MCP entry through Ruler (`distribute_via_ruler`).
5. Install the Claude Code hooks, unless `--no-hooks` is passed.
6. Print where each tool's MCP config was written and remind the user to restart the tool.

### Remote mode

1. Get a personal API key: `--api-key`, a prompt, or the device flow.
2. Find the repo's connection id. A previous `.context/agentops-remote.json` for the same server is reused, so re-running does not register a duplicate. Otherwise the CLI matches the git remote against `GET /repos`, or registers the repo.
3. Generate `AGENTS.md` if missing.
4. Add `.context/` and other generated paths to `.gitignore` (`ensure_gitignore_entries`). This runs every time, because the next step writes a live key.
5. Write `.context/agentops-remote.json` with `server_url`, `connection_id` and `api_key`.
6. Run Ruler. When the marker exists, Ruler's stdio entry is skipped and the remote entries are written instead.

The steps above are the **full** connect, which only AgentOps' own checkout gets (its origin is `Jesus-Glez60/agentops`), or any repo with `--full`.

The marker is how everything else knows the repo is remote: `docs/vault-protocol.md`, the local-notes hook, and `agentops usage sync` all check it. It holds a credential, so it must stay out of git. A checkout that uses an SSH config host alias once failed to match its connection (gotcha 53975). Merging a local-only stub into a GitHub connection leaves the marker pointing at a dead id (gotcha 54243). Since milestone 5, a reconnect checks the marker's id against `GET /repos` and resolves again if it is gone.

### Guest connect (every other repo)

Milestone 5 item 2 (`agentops-cli/src/guest_connect.rs`). In any other repo, a client's above all, `connect --remote` changes no tracked file, so `git status` stays clean:

1. For `.context/` and `.claude/settings.local.json`, `git check-ignore` decides. A path that isn't ignored is appended to the repo's `info/exclude` (`git rev-parse --git-path`), which is local and never committed. A path git already tracks aborts the connect before anything is written. A plain folder, or one nested inside another repo, gets no exclude.
2. The connection is resolved as above. A remote that matches no connection is registered **memory-only** (`register_repo` with a `local_id`): notes, similarity and keyword/meaning recall work, `Affects` links don't.
3. The marker is written, then the hooks go to `.claude/settings.local.json`, the same entries the full connect puts in `settings.json`.
4. The MCP entry is written user-level, the only write outside the repo.

`AGENTS.md`/`CLAUDE.md`, `.gitignore` and Ruler are never touched. `agentops memory-off` sets `autoMemoryEnabled: false` in `settings.local.json`, or with `--user` in `~/.claude/settings.json`. Settings files that aren't valid JSON are left alone and the command fails, instead of being replaced.

### connect.sh

`GET /connect.sh` on the server returns a shell script for one-command setup: `curl -fsSL <server>/connect.sh | sh` (`agentops-core/crates/agentops-server/src/connect_sh.rs`).

- It installs the CLI if missing, then runs `agentops connect --remote <url> --agents ... --yes --device-login`.
- `<url>` comes from the request's own `Host` and `X-Forwarded-*` headers. The user just fetched the script from that address, so it is reachable.
- The route is unauthenticated and the script contains no key; `--device-login` gets one through the browser.
- The body is wrapped in a function called on the last line. A truncated download then fails to parse instead of running half a script.

## Instructions, skills and agents

### Prompt pack

`agentops-core/prompts/` is the single source:

| Folder | Contents | Becomes |
| --- | --- | --- |
| `skills/` | 16 skill stubs, each `@include`-ing its body from `shared/` | `<agent dir>/skills/<name>/SKILL.md` |
| `shared/` | skill bodies | inlined into skills |
| `agents/` | `plan-council-member`, `plan-council-chair`, `ponytail-auditor`, `vault-archivist` | Claude Code subagents |
| `docs/` | `vault-protocol.md` | referenced by `save-note`, `wrap` and `vault-archivist` |

`agentops-ruler-bridge` embeds the folder at compile time (`include_dir!`). A prompt edit therefore needs a rebuild of that crate before `install` or `connect` picks it up (gotcha 54118).

### Ruler bridge

`build_ruler_dir` (`agentops-core/crates/agentops-ruler-bridge/src/lib.rs`) writes a `.ruler/` source directory, then runs the pinned `npx @intellectronica/ruler@0.3.44` (`RULER_VERSION`) to fan it out to the selected agents. Findings recorded in the module doc and gotchas:

- Ruler does not resolve `@include`, so the bridge inlines includes itself; every distributed file is self-contained.
- Ruler recognizes skills only as `skills/<name>/SKILL.md`, not flat `.md` files.
- `.ruler/mcp.json` is a deprecated fallback in 0.3.44; it works but warns (gotcha 54073).
- Ruler's frontmatter parser rejects an unquoted description containing a colon (gotcha 54074).
- Running `ruler apply` directly touches every supported agent, not only the selected ones; use `install`/`connect` (gotcha 54122).
- The `.claude/`, `.cursor/`, `.gemini/` and `.ruler/` copies in a repo are outputs. Editing them by hand gets missed or overwritten (gotcha 54113).
- Claude Code merged slash commands into skills; do not ship both with one name (gotcha 53973). `$ARGUMENTS` works only in commands and shows up literally in a skill (gotcha 53961).

Ruler needs Node, because it runs through `npx`. Bump `RULER_VERSION` deliberately; never use `@latest`.

### AGENTS.md

`agentops-agents-md` generates `AGENTS.md`, the vendor-neutral instructions file. It is plain Markdown with no `@include`, because only Claude Code resolves that. It carries a `NOTES_PATH:` marker so agents write notes where ingestion reads them ([06-notes-and-consolidation.md](06-notes-and-consolidation.md), "Notes folder resolution").

`init_agents_md` (MCP tool, Full mode) and `agentops install` overwrite `AGENTS.md` and the matching `CLAUDE.md` completely. There is no preserved hand-edit section, and the files are untracked (gotcha 54112). `connect` reuses an existing `AGENTS.md` instead of regenerating it.

## Claude Code hooks

`agentops connect` adds hooks to Claude Code's settings for seven events, unless `--no-hooks` is passed. They were opt-in (`--with-hooks`) until milestone 1. They're on by default now because subagents never load `AGENTS.md`/`CLAUDE.md` and plan mode never calls MCP tools on its own, so hooks are the only reliable way memory reaches the agent (`docs/engineering/spikes/m1-hooks.md`). Each runs `agentops hook-capture --event <name>` (`agentops-core/crates/agentops-cli/src/hook_capture.rs`).

The store logic for every event lives in one place, `agentops_mcp::hooks::handle` (`agentops-core/crates/agentops-mcp/src/hooks.rs`). `hook_capture` only decides where it runs:

- **Local** (no `.context/agentops-remote.json`): it opens the repo's store and calls `handle` directly.
- **Remote** (marker present): it posts the payload, minus `cwd` and `transcript_path`, to the server's `POST /repos/{id}/hooks/{event}` with the marker's API key. The server runs the same `handle` against the connection's store, and `.context/graph.db` is never opened. A network error, a timeout (`REMOTE_HOOK_TIMEOUT`, 3 s) or a non-2xx reply logs one stderr line and exits 0 with no context, so a session never stalls.

`hooks::is_relevant` filters on the client first, so an ordinary tool call (a `Read` with small output, a `PreToolUse` with nothing to warn about) costs no store open and no HTTP call. Before milestone 1, hooks always opened the local store, even in a remote-connected repo, so their events and pins went to an orphaned local graph.

| Event | What it does |
| --- | --- |
| `PostToolUse` | Categorizes the call (edit, git, web) into `session_events`. Every file read becomes a `file_read` event and every `Grep`/`Glob` a `search` event (milestone 3b, F68). A read is the `Read` tool or a Bash command that only reads one file (`cat`, `head`, `tail`, `sed -n`). The event stores the repo-relative path or the pattern, never content. `hook-capture` works out that path (`agentops_rel_path`) because the server never sees the repo root, and a read outside the repo isn't recorded. A subagent's events carry its `agent_id`. A read or edit inside the repo is also a work cue (see "Recall during work" below). If output is too large to inline, redacts it and stores it as a `ToolOutput` node, findable by search and `fetch_content`. |
| `PostToolUseFailure` | Records an `error` event; the error, unless it was an interrupt, is a work cue. `PostToolUse` never fires on failure (gotcha 53971). Claude Code doesn't send validation errors or permission denials here. |
| `UserPromptSubmit` | Records `user_intent` and injects pinned nodes (their own `PIN_BUDGET`; a pinned rule shows as its rule line alone, see [06-notes-and-consolidation.md](06-notes-and-consolidation.md), "Pins"), plus the task-start flow in plan mode or a recall seed otherwise (see "Task-start flow and recall seed" below). Skips the `<task-notification>` prompts Claude Code sends when a background task finishes. Installed with a 10 s timeout. |
| `SessionStart` | On startup, resume or compact, injects the session guide (`session_guide::build`) and pinned nodes. On compact it also records `context_reset`, so work-time recall starts over in the main context. |
| `PreToolUse` | Warns about likely secret exposure. Warning without blocking needs `hookSpecificOutput` JSON, not stderr (gotcha 53971). On `ExitPlanMode`, checks the plan's required sections (see below). Denies a `Write`/`Edit`/`MultiEdit` into Claude Code's memory directory (`~/.claude/projects/<project>/memory/`) and redirects it to `add_note` with a `memory-kind:semantic|episodic|procedural` tag, recorded as `memory_write_redirected`. Reads of MEMORY.md stay allowed until milestone 2 imports them. Never blocks anything else (ADR 0049). |
| `SessionEnd` | Starts a detached `agentops usage sync --transcript` for the ending session, and a detached `agentops consolidate --session <id>` (milestone 4, [ADR 0056](adr/0056-consolidation-proposals.md); its output goes to `.context/consolidate.log`). |
| `SubagentStart` | For `Explore`, `Plan` and the `plan-council-*` agents (`hooks::SEEDED_AGENTS`), injects pinned nodes and a recall seed built from the session's latest user prompt, within `SUBAGENT_SEED_BUDGET` (2,000 characters) plus `PIN_BUDGET` (3,000) for the pins. Subagents load no `CLAUDE.md`/`AGENTS.md`, so this is their only memory channel. The seed is recorded as shown in that subagent's context only (`recall_injected` with its `agent_id`), because the main session hasn't seen it. Installed with a 10 s timeout. |

`write_claude_hook` merges into existing settings: other tools' hooks and the user's own groups on the same event survive. An earlier version wrote the wrong schema, and no installed hook ever fired (gotcha 54240). Claude Code's `PostToolUse` `tool_response` is an object with `type` and `text`, not a bare string (gotcha 53972).

### Task-start flow and recall seed

Milestone 1 makes the prompt hook do recall itself, because plan mode never calls AgentOps tools on its own (`agentops_mcp::hooks`, `user_prompt_submit`). Each prompt is recorded as a `user_intent` event whose `tool_name` is `user_prompt`, or `user_prompt:plan` when Claude Code reports `permission_mode: "plan"`. The next prompt reads that to tell whether a planning episode has already started.

| Prompt | Injected (pins always first) |
| --- | --- |
| Plan mode, first prompt of an episode (the previous prompt wasn't in plan mode) | The task-start flow directive (`agentops-core/prompts/shared/plan-flow.md`, compiled in), then a recall seed of up to `PLAN_SEED_MAX_ITEMS` pointers |
| Plan mode, later prompts | One-line reminder; the flow never restarts mid-episode |
| Any other mode | Up to `DEFAULT_SEED_MAX_ITEMS` recall pointers not already shown this session, or nothing if there are none |

The directive tells the agent, in order: use the `plan` skill; recall first; run `library-docs` for every library touched; ask at most `MAX_PLAN_QUESTIONS` pointed questions if the task is ambiguous; run `audit-plan`; present with "Research performed" and "Council verdict".

The seed comes from `agentops_retrieval::recall_seed_with` (`agentops-core/crates/agentops-retrieval/src/recall.rs`). In a local hook process it is lexical and graph only, and no embedding model runs: `hook-capture` calls `hooks::handle`, which passes no embedder. On the server (`POST /repos/{id}/hooks/{event}`), `hooks::handle_with` passes the warm `LocalEmbedder`. The prompt is embedded, and its nearest memories (cosine ≥ 0.55) are fused with the keyword ranking. That half is dropped if it takes over 250 ms ([ADR 0052](adr/0052-the-server-may-embed-the-prompt-in-a-hook.md)). The prompt is cut into keywords, and each keyword goes to `GraphStore::search_lexical` per memory kind (Gotcha, Decision, Note). Generated doc sections never reach a seed: digests matched every prompt and took three of eight slots on each (F53, milestone 3a item 2). They go separately because SQLite's FTS5 query ORs terms while Postgres's `plainto_tsquery` ANDs them. Each matched keyword weighs `ln(1 + memories / memories matching it)` (`GraphStore::count_lexical`, `count_nodes`), so a rare word outranks a common one. Identifier-shaped words are matched exactly against symbol names and followed one hop over `Affects` edges to their notes. Each memory that qualifies is then followed one hop over `SimilarTo` edges (reason `similar to "<title>"`), and `NeedsReview` memories never appear. Results are fused by reciprocal rank, reduced-prominence nodes rank lower, and pinned nodes are left out because the pin block already shows them. Each pointer is one line: kind, title, node id, and why it matched.

Every pointer sent is recorded as a `recall_injected` event (`ids 1,2,3`), so ordinary prompts never repeat a pointer within a session. Everything injected fits in `PLAN_FLOW_BUDGET` (4,000 characters), and pointers are dropped first. All of these limits are constants in `agentops-mcp/src/budget.rs`.

### Recall during work

Milestone 3b (`agentops_mcp::work_recall`). The file the agent opens or edits, and the errors it hits, recall memories without any tool call, in the main session and in subagents.

- **Cue.** Each file read or edited inside the repo, and each failed call, is a cue. `resolve_cue` turns it into scored candidates ([05-retrieval.md](05-retrieval.md), "Recall during work"). A cue that reaches anything records a `work_cue` event (`id:score`, top 10). A file cue that reaches nothing records an empty one.
- **Activation.** Each context has its own activation, summed over its last 20 cues with decay 0.85 ([ADR 0053](adr/0053-session-activation-from-recorded-cues.md)). A context is the main session or one subagent (`agent_id`). A plan- or default-mode prompt counts as a cue in the main context.
- **What gets shown.** At most `WORK_RECALL_MAX_ITEMS` (1, the author's volume ceiling from milestone 3b revision 2) memories that this cue reached and whose activation is at least `INJECT_MIN_SCORE`, within `WORK_RECALL_BUDGET` (800 characters). They go out as `PostToolUse`/`PostToolUseFailure` `additionalContext` as short lines, `- [kind] Title (id N)`, under one fixed header (`WORK_RECALL_HEADER`). The header names neither the file nor the error, since it rides on the tool call that triggered it, and the lines leave out the match reasons that prompt-time pointers carry. Together those were about half of every injection (milestone 3b revision 2).
- **Only a file's first cue injects.** A re-read or a later edit of a file already cued in this context re-records that file's earlier candidates (it adds activation) and shows nothing, without resolving the file again. A compaction re-arms the main context. Before this rule, re-reading a hub file drained its mention list three pointers at a time (gotcha 235636, milestone 3b revision 1).
- **Never twice in a context.** A memory already shown in this context (prompt seed, subagent seed, earlier injection) is skipped; a compaction resets the main context. Parallel tool calls no longer race (F71, milestone 4): each claims its lines in `recall_shown` before showing them and shows only what it won.
- **A ceiling.** After `MAX_INJECTED_PER_CONTEXT` (30) work-time pointers in the main session, only error cues inject. A subagent stops at `SUBAGENT_MAX_INJECTED` (8), error cues included. That covers its whole life, since a subagent never gets a `context_reset`. It already has a `SubagentStart` seed, and subagents carried 87% of the replayed volume (milestone 3b revision 1).
- **Silence is normal.** A cue that shows nothing records no `recall_injected`.
- **Measuring it.** `agentops work-recall-eval --replay` runs past sessions through the same code via `POST /repos/{id}/work-recall/replay`, against an in-memory log. `--sources no_affects|no_mentions` turns one file-cue source off, for both cases and replay (the ablation, milestone 3b revision 1).
- **Off switch.** `AGENTOPS_WORK_RECALL=off` turns injection off and leaves prompt-time recall alone. `agentops connect --no-work-recall` sets the variable in `.claude/settings.json`'s `env`, and connecting again without the flag removes it. `hook-capture` forwards the setting as `agentops_work_recall: false` in the payload, so a remote server honors it too. Reads and cues are still recorded. There is no separate repo config file: AgentOps has none.

### Memory scope

Milestone 4 item 3. Some memories hold only on one machine: one server's deploy setup, or one SSH alias. In milestone 3c, 4 of the 6 non-progress noise rows were these, and all came from error cues.
- **`client_host`.** `hook-capture` adds this machine's hostname to every payload it forwards, normalized by `normalize_host`: trimmed, lowercased, cut at the first dot (macOS reports `name.local`). It's omitted when `hostname` fails. Nothing else from the environment is sent, neither the OS username nor environment variables (tested). The server normalizes it again.
- **`applies_to`.** A memory's hosts: a JSON list in `nodes.applies_to`, NULL for everywhere. Set by `add_note`'s `applies_to` or the `set_memory_scope` tool (an empty list clears it). Kept across rescans like the class. The hosts are client hostnames, the machines Claude Code runs on. A memory scoped only to a server that never runs Claude Code is therefore never recalled by an error cue; the author chose that meaning, 2026-10-24.
- **Recall.** An error cue drops a memory whose `applies_to` is set and doesn't contain the payload's `client_host`. Without a `client_host`, nothing is skipped. Prompt seeds, subagent seeds and file cues still show it, labelled ` — on <hosts>`, since you may be working on that machine's setup from another one.
- **Backfill.** `agentops classify-memories --scope [--host NAME]...` lists every memory and proposes, for each one whose title or body names a candidate host as a whole word (case-insensitive), `applies_to` = the names found.
  - The candidates are this hostname, the concrete `Host` aliases in `~/.ssh/config`, and `--host`. They are read and matched on this machine and never sent.
  - The proposals go to `.context/classify-memories.scope.{json,md}`, with what each means for error cues from here.
  - `--apply` sends exactly the saved file to `POST /repos/{id}/memories/scope`.
- **Replay.** `work-recall-eval --replay ... --client-host NAME` replays as if the cues came from that host.

### Recording use: "followed"

Milestone 4 item 2. A lookup of a memory the context was shown is the one use signal milestone 3c found that separates useful recall from noise (gotcha 340675). It is recorded; no weight changes from it.
- **What counts.** A `PostToolUse` for `fetch_content` (its `id`), `related_context` (its `symbol_id`) or `get_symbol`, where the memory is one this context was shown. `get_symbol` takes a name, so it counts when the name equals a shown memory's title exactly.
- **Shown** means a row in `recall_shown` under the context's claim key (a subagent's id, or `main#<n>`), else a current pin.
- **Recorded as** a `memory_followed` session event: `id N source S context C`, with `node_id` N. The source is the claim's: `prompt_seed`, `subagent_seed`, `file_cue`, `error_cue`, or `pin`.
- **Subagents.** These three tools are forwarded from subagents too, unlike other agentops calls (`is_relevant`), so a follow of a subagent seed lands in the subagent's context. This needs the reinstalled CLI.
- **Reading it.** `agentops recall-stats [--remote] [--since YYYY-MM-DD]` (route `GET /repos/{id}/recall/stats`) prints, per source and memory class, the memories shown, the ones followed and the rate. It counts distinct `(session, context, memory)` triples and leaves out `recall-eval-*` sessions. Pins have follows but no shown count. Since milestone 5 it also splits sessions into **meta** (one that read or edited AgentOps' own memory or recall code, `recall_stats::META_PATHS`) and **ordinary**, and gives ordinary sessions' follows per source; replays and work-evals write no sessions. Meta is undercounted, since only some sessions record their file events (gotcha 195365).
- **When to revisit reinforcement:** at 200 follows from at least 20 ordinary sessions (`handoffs/m3b-eval.md`).

### Personal memories

Milestone 5 item 4 (`agentops-heavy-api/src/personal.rs`). Facts about the person rather than a repo (preferences, how they like to work) live in one **personal connection per user account**, recalled in every repo they work in.

- **Where.** Its own tenant, `u<user_id>`, never the user's org tenant, with connection id `personal` (a `local:` connection, created by the first write). Org-scoped lookups, listings, dashboards and reindexing resolve against the org tenant, so none of them can reach it, even given its id, and it follows the user across org switches.
- **Reaching it.** Only from the caller's own user id: `path: "personal"` on `/mcp` (session or personal API key), `add_note personal: true`, the curation and retitle routes with `personal` as the repo id, and the hook route's authenticated user. Reads never create it.
- **Recall.** The server passes it to `hooks::handle_with_personal`. Its pins join the pin block after the repo's (`pinned_block`; repo pins win the shared `PIN_BUDGET`). Prompt and seeded-subagent seeds add up to `PERSONAL_SEED_MAX = 2` of its memories after the repo's own, labelled `[personal]`, so a cap trims them first. They are claimed in the personal repo, so `recall-stats` classes them there and a `fetch_content` with `path: "personal"` is recorded as a follow there. Work-time cues (files, errors) never use it.
- **Import.** `agentops import-memory --personal --memory-dir <user-level dir> --project <a connected repo>` writes into it.
- **Local mode** has no personal connection: `add_note personal: true` fails there, and nothing personal is recalled.

### Importing MEMORY.md

Claude Code's own auto-memory (MEMORY.md) is imported once with `agentops import-memory` ([06-notes-and-consolidation.md](06-notes-and-consolidation.md), "Importing Claude Code's MEMORY.md"), then turned off per project with `agentops memory-off`, which writes `autoMemoryEnabled: false` to `.claude/settings.local.json` (verified in the milestone 2 spike: the session's system prompt loses its memory section). Milestone 1's redirect of memory writes to `add_note` stays on as a backstop.

### Plan enforcement

A plan presented in plan mode must have a "Research performed" heading and a "Council verdict" heading (`check_presented_plan` in `agentops_mcp::hooks`). The spike showed that Claude Code 2.1.286 passes the full plan to `PreToolUse` on `ExitPlanMode` as `tool_input.plan`, and obeys a deny. The first plan in a planning episode that lacks a section is denied with a reason naming the missing step: recall, or run `audit-plan`. That is recorded as a `plan_nudge` event. The next attempt goes through with a top-level `systemMessage` warning to the user (`plan_presented_incomplete`), so it never loops (`ENFORCE_MAX_NUDGES`). A planning episode starts after the last ordinary-mode prompt. Only headings count, not prose. If `tool_input.plan` is missing, the plan goes through untouched. The existing `PreToolUse` hook already runs for every tool, so this needs no new hook entry. ADR 0049 records why `PreToolUse` may block here.

### Council agents

`plan-council-member` and `plan-council-chair` (`agentops-core/prompts/agents/`) list every read-only AgentOps tool by full name (`mcp__agentops__<tool>`) in their `tools:` frontmatter, next to `Read`/`Grep`/`Bash`. In Claude Code, an explicit `tools:` list excludes any MCP tool it doesn't name, so before milestone 1 the Completeness-checker couldn't call the `list_gotchas`/`related_context` its persona asks for. The member prompt tells it to query the graph per file and symbol before grepping. A test in `agentops-cli` renders the prompt pack through `agentops-ruler-bridge` and checks the list against both tool registries' `read_only_hint`: every read-only tool is listed and no write tool is (`council_agents_list_every_read_only_agentops_tool_and_no_write_tool`).

### Command compression

`--compress-commands` adds `--compress` to the `PreToolUse` hook. It rewrites known-noisy Bash commands, such as test runners, `git diff`/`git log` and `grep`/`rg`, through `updatedInput`, so output is compressed before it reaches context (`agentops-core/crates/agentops-cli/src/compress_output.rs`). It is opt-in because it changes the command that runs, a bigger trust step than observing.

The rewrite pipes through the compressor and preserves the original exit status through bash's `PIPESTATUS`; a plain pipe would report the compressor's status (gotcha 53900).

## Usage tracking

| Source | Entry point | Stored as |
| --- | --- | --- |
| Claude Code transcripts | `agentops usage sync` (manual or the `SessionEnd` hook) | `session_usage`, transcript-based |
| Agent self-report | MCP `report_usage` (Full) | `session_usage`, agent estimate; never replaces a transcript row |
| AgentOps's own LLM calls | `UsageRecorder` in `agentops-llm` | `llm_usage` spend ledger |

`agentops usage sync` parses `~/.claude/projects/<sanitized path>/<session id>.jsonl` (`agentops-core/crates/agentops-cli/src/usage.rs`):

- It needs `--path` explicitly, because Claude Code's directory-name sanitization cannot be reversed when the real path contains a dash.
- It writes locally, or pushes to the server when `.context/agentops-remote.json` exists (`usage_sync_command`).
- Transcripts repeat each message's usage on every content-block line, and subagent transcripts live in a subdirectory; both are handled (gotcha 54236).
- Claude Code's session UUID is not necessarily the `session_id` an agent passes to tools. The server joins them heuristically.

`agentops usage report` summarizes a session or all sessions, with options for reset points and hand-off thresholds (`usage_report.rs`).

Costs come from `agentops-llm`'s `pricing` table (`rates_for`, `cost_estimate_usd`). Its rates are matched by model-name substring and are known to overstate current Opus pricing about 3.75× (gotcha 54234).

## Adding a new tool

1. Confirm the tool's user-level MCP config path and entry shape from its own current docs. The project's convention is to verify, not assume.
2. If it uses the common `{"mcpServers": {...}}` JSON shape, add arms to `user_level_mcp_config_path` and `json_remote_entry` in `main.rs`. Otherwise write a dedicated writer.
3. Add it to `select_agents`' named list if it should appear in the picker.
4. Check which Ruler agent id distributes its instructions and skills.
5. Hooks are Claude Code only today.

## Open questions

- None. What a connected tool can call per access mode is in [07-mcp-and-rest-contracts.md](07-mcp-and-rest-contracts.md) (ADRs 0039, 0041).
