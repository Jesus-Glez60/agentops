# Milestone 1 handoff: task-start flow

Written 2026-10-14 when milestone 1 of the cue-driven memory requirement was merged. It feeds the milestone 2 spec (MEMORY.md import). Facts only, each with a file path, commit or command output. Anything not verified is marked **unverified**.

## 1. What shipped

All on `main`, deployed to the hosted server by the push-to-main autodeploy. The server's CI gate ran on `a998c7e`, and `POST /repos/{id}/hooks/nosuchevent` answered `{"error":"unknown hook event 'nosuchevent'"}` once it was live.

| Item | Commit | What |
| --- | --- | --- |
| 0 | `2c7e453` | Spike findings, `docs/engineering/spikes/m1-hooks.md` |
| 1 | `25c4591` | Hooks route to the server on remote-connected repos; hooks on by default |
| 2 (+2b) | `bcf44c6` | Plan-mode flow directive and recall seed on `UserPromptSubmit`; seed outside plan mode too |
| 3 | `6dd348b` | Council agents get the read-only graph tools; `SubagentStart` seeds subagents |
| 4 | `6f9a95f` | Note tags stored in the graph; `list_gotchas` `query`/`tag`; `library-docs` loads library gotchas |
| 5 | `1fe8f17` | Pointed questions in the plan flow, capped at 3 |
| 6 | `97a2da3` | A plan missing "Research performed"/"Council verdict" is sent back once |
| 7 | `404be45` | Writes to Claude Code's MEMORY.md are redirected to `add_note` |
| follow-ups | `c1cf6ab`, `a998c7e`, `591bbed` | Postgres test for the prompt hook; recall relevance fixes from measuring against real graphs |

### Deviations from the spec

| Change | Why | Approved by |
| --- | --- | --- |
| One route per hook event (`POST /repos/{id}/hooks/{event}`) instead of `session-events`/`session-context`/`recall` | One HTTP call per hook, not ~4. Both paths run the same `agentops_mcp::hooks::handle`, so they can't drift. | Author, 2026-10-14 (asked before item 1) |
| Recall seed on non-plan prompts too (item 2b): new pointers only, at most 5 | The author's goal is AgentOps as the only memory, start to finish | Author, 2026-10-14 |
| Item 7 added: MEMORY.md writes redirected to `add_note` | Same goal | Author, 2026-10-14 |
| Rules that must always hold were deferred to a later milestone, with requirements written down (plan file, "Deferred: procedural memory") | Needs its own spec | Author, 2026-10-14 |
| Item 6 uses a `PreToolUse` deny on `ExitPlanMode`, not a Stop or `PermissionRequest` hook | Spike Q3: the deny is obeyed on 2.1.286, and a rejected plan fires no Stop | Author, in the revised plan |
| Hooks installed by default (`--no-hooks` opts out) | The spike found this repo had never had them, and every item depends on them | Author, in the revised plan |
| No `agentops-api` parity routes | Local hooks never use HTTP; `usage/sync` already lives only in `agentops-heavy-api` | Author, in the plan |
| Tags stored as a final `<!-- agentops:tags ... -->` line in node content, not a column | No two-store migration | Author, in the plan |
| The spec's integration test (agentops-server in process with Postgres, a marker and the user-prompt hook end to end) wasn't built as one test. It's covered in pieces: the route over a SQLite store (`dashboard::tests::hook_events_round_trip_through_the_connections_store_and_never_cross_tenants`), the CLI forwarding to a fake HTTP server (`hook_capture::tests::a_remote_repo_forwards_the_payload_without_local_paths_and_returns_the_servers_output`), the hook logic on Postgres (`hooks::tests::the_prompt_hook_records_and_recalls_against_postgres`), and a live check on the real server (section 3) | Building agentops-server in a test needs accounts/teams setup that already exists in heavy-api's tests | **Not explicitly approved**; my call, flagged here |
| Subagent seeds aren't recorded as "already shown" | A subagent's context isn't the main session's. Recording them would hide pointers from the main session. | **Not explicitly approved**; my call |
| `Plan` added to the seeded subagent types | Plan mode's own design phase uses it | **Not explicitly approved**; my call |
| Recall relevance rules beyond the spec: the repo's own name isn't a keyword, a 2-keyword floor for 3+ keyword prompts, doc-section digests at 0.25 weight, ranking by matched cues first | Measured seeds were mostly noise without them (findings F53) | **Not explicitly approved**; my call, from measurements |
| The spec test "a default-mode prompt emits only pinned notes, exactly as today" became pins plus new pointers | Follows from 2b | Author (2b) |
| `hook_output` puts `systemMessage` at the top level; it was nested in `hookSpecificOutput` | Claude Code reads it as a top-level field. **Unverified live**: inferred from the hooks reference, which says Elicitation "discards `systemMessage` and `continue`" next to `hookSpecificOutput` | **Not explicitly approved**; my call |

### Spike findings and later hook learnings

See `docs/engineering/spikes/m1-hooks.md`. Learned after the spike:

- Hook config edits take effect mid-session. The probes and the real hook install both worked without a restart.
- In a live session after install, the hooks recorded a 22,686-character Bash output capture and a `Write` on the server (`get_session` for session `ec13d098-1fb4-4324-a064-372d06a193bd`, 2026-10-15 02:05 UTC).

## 2. Interfaces milestone 2 will reuse

**Lexical recall.** Crate `agentops-retrieval`, `src/recall.rs`:

```rust
pub fn recall_seed(store: &dyn GraphStore, repo: &str, query: &str, limit: usize, exclude: &HashSet<i64>) -> Result<Vec<RecallPointer>>
pub struct RecallPointer { pub node_id: i64, pub kind: NodeKind, pub title: String, pub reason: String }
impl RecallPointer { pub fn render(&self) -> String }
pub const RECALL_KINDS: &[NodeKind] = &[Gotcha, Decision, Note, DocSection];
```

It searches node kinds Gotcha, Decision, Note and DocSection: one `GraphStore::search_lexical` call per keyword (at most 8 keywords) and kind, plus `search_exact` on identifier-shaped words, followed one hop over `Affects` edges. Pinned nodes and `exclude` are skipped. No embedder.

**Hook routing.** `agentops-core/crates/agentops-cli/src/hook_capture.rs`, `dispatch(event, payload)`. If `remote_marker::read_remote_marker(cwd)` returns a marker, the payload (minus `cwd`/`transcript_path`) goes to `call_remote`, which POSTs to `{server_url}/repos/{connection_id}/hooks/{event}` with `Bearer {api_key}` and `REMOTE_HOOK_TIMEOUT` (3 s). Otherwise `agentops_mcp::hooks::handle(store, repo, event, payload)` runs locally. To add a store-touching event:

1. Add its name to `agentops_mcp::hooks::STORE_EVENTS`.
2. Add a match arm in `hooks::handle`.
3. If most occurrences need nothing, add a cheap filter to `hooks::is_relevant`.
4. Add it to the `HOOKS` table in `maybe_install_claude_hooks` (`agentops-cli/src/main.rs`).

The server route accepts any name in `STORE_EVENTS`, with no other change.

**Server route as built.** `POST /repos/{id}/hooks/{event}` (`agentops-heavy-api/src/dashboard.rs`, `hook_event_json`). Body: the hook payload. Reply: `{"output": <hook stdout JSON or null>}`. Unknown event or foreign connection: 404. Auth: session or personal API key. `event` ∈ `posttooluse`, `posttoolusefailure`, `userpromptsubmit`, `sessionstart`, `pretooluse`, `subagentstart`. No other routes were added.

**`list_gotchas`.** Inputs `path*`, `session_id`, `query`, `tag`. `query` means every whitespace-separated word appears in the title or body, case-insensitive. `tag` means an exact, case-insensitive match against the node's tags. The two combine with AND, and filtering happens in Rust over `nodes_by_kind`.

**Tags on a note.** Frontmatter `tags` → `VaultNote.tags` → `ingest_vault` appends `\n\n<!-- agentops:tags a, b -->` to the node content (`agentops_notes::content_with_tags`). It's read back with `agentops_notes::tags_of(content)`, which reads only the last line. `TAGS_MARKER` is `"<!-- agentops:tags "`. Notes ingested before `6f9a95f` get their tags on their next re-ingest. **Unverified**: whether the hosted reindex has re-ingested this repo's notes since the deploy.

**Other shared pieces.** Constants live in `agentops-mcp/src/budget.rs`: `PLAN_FLOW_BUDGET` 4000, `PLAN_SEED_MAX_ITEMS` 8, `DEFAULT_SEED_MAX_ITEMS` 5, `SUBAGENT_SEED_BUDGET` 2000, `MAX_PLAN_QUESTIONS` 3, `ENFORCE_MAX_NUDGES` 1. The session event kinds and tool names are public constants in `agentops_mcp::hooks`: `PROMPT_TOOL`, `PLAN_PROMPT_TOOL`, `RECALL_INJECTED` (`ids 1,2,3`), `PLAN_NUDGE`, `PLAN_PRESENTED_INCOMPLETE`, `MEMORY_WRITE_REDIRECTED`, and `hooks::last_prompt(store, repo, session_id)`.

## 3. Measurements

**Recall latency.** The whole `UserPromptSubmit` hook (process start, store or HTTP, recall), 50 prompts each, from `python3 latency.py <agentops binary> <repo dir> <mode>`. The script is in the session scratchpad and sends 50 fixed prompts on stdin with fresh session ids. Client: release build at `a998c7e` on a macOS laptop; server at `a998c7e` (before the `591bbed` ranking change, which only reorders results).

| Path | Mode | p50 | p95 | Target |
| --- | --- | --- | --- | --- |
| Local SQLite (copy of this repo's `.context/graph.db`) | default | 14 ms | 17 ms | p95 < 300 ms |
| Local SQLite | plan | 13 ms | 16 ms | p95 < 300 ms |
| Remote (laptop → `api.agentops.dedyn.io`, Postgres) | default | 152 ms | 220 ms | p95 < 1 s |
| Remote | plan | 70 ms | 82 ms | p95 < 1 s |

A first remote default-mode run measured p50 174 ms / p95 340 ms, probably with cold caches. The table shows the rerun.

**Ranking after `591bbed`, verified live** (deployed 2026-10-15 02:15 UTC per the autodeploy log). For "the github webhook signature check fails with 401", the server's seed now puts gotcha 58357 ("Every GitHub webhook delivery to the hosted server gets 401 invalid signature...") first. Before, it ranked third.

**Graph size at measurement.** Local `.context/graph.db` (last scanned 2026-10-05): symbol 3149, file 357, gotcha 91, decision 50, note 10, doc_section 9 (`sqlite3 ... group by kind`). Server: Symbol 3773, File 375, Gotcha 172, Decision 89, Note 35, DocSection 9, ToolOutput 1 (`GET /repos/Jesus-Glez60--agentops/graph`, 2026-10-15).

**Manual checks from the spec.**

| Check | Result |
| --- | --- |
| 1. Plan mode, "add a --json flag to `agentops status`": seed, library-docs, audit, both sections | **Not run as a full session.** At the hook level, the flow directive is injected (tested). The seed for this prompt is empty against both graphs after the relevance fixes, because nothing recorded is about it. |
| 2. "make search better": at most 3 questions, open assumptions | **Not run.** The prompt rules are in place (`1fe8f17`); agent behavior is unverified. |
| 3. Council transcripts show agentops calls before Grep | **Not run.** The rendered agents carry the tools (test), but behavior is unverified. |
| 4. Plan without "Council verdict": one nudge, then a warning | **Not run end to end with the shipped hook.** The deny mechanism was demonstrated live in the spike with a probe hook; the shipped logic is unit-tested (`a_plan_missing_a_section_is_sent_back_once_then_let_through_with_a_warning`). |
| 5. Evidence-session replay | **Not run.** The requirement's evidence transcripts were never attached. |

**Evidence-session replay.** Not done, for the reason above.

## 4. Current state of memory writing

| Path | Inputs | Classification | De-duplication |
| --- | --- | --- | --- |
| `add_note` MCP tool, local stdio or hosted `/mcp` (`agentops-mcp/src/tools.rs`, `notes.rs`) | `path*`, `title*`, `body*`, `note_type` (gotcha/decision/knowledge), `tags`, `with_embeddings`, `session_id`, `notes_path` (refused on hosted) | `HeuristicClassifier` when `note_type` is omitted | Writes `<slug-of-title>.md`; the node upserts on natural key (kind, `vault:<source path>`, title), so the same title updates in place. No content-level dedupe. |
| `ingest_notes` MCP tool / `agentops ingest-notes` | `path`, notes folder, `--llm-classify`/`--llm-match` on the CLI | Frontmatter `type`, then the parent folder, then the heuristic (or LLM) | Same natural-key upsert |
| `agentops note` CLI | `--kind`, `--tags`, `--with-embeddings`, title, text | Heuristic when `--kind` is omitted | Same |
| Vault files under `NOTES_PATH` (local) or `repo-checkouts/.notes/<connection>` (hosted, ADR 0045) | Markdown with `title`/`type`/`tags` frontmatter | As above | Re-ingested on every scan/reindex (`bump_confirmed_at: false`) |

Hosted writes go through `/mcp` only (`docs/vault-protocol.md`). In this repo, `.claude/hooks/block-local-notes.sh` blocks a local note write when the remote marker exists.

**Symbol matching for a new note.** `WordBoundaryMatcher` (`min_name_len` 4) finds symbol names in the body at word boundaries, skipping names that occur too often. Each match becomes an `Affects` edge from note to symbol. A note that matches no symbol is still a node with no edges: `list_gotchas`, lexical search and recall keywords still find it, but `related_context`/`get_symbol` never surface it. Notes written before the first scan never attach (gotcha 54035). The hosted reindex re-matches them.

**Curation states.** `NodeProminence`: `Full` (default), `Reduced` (with a required `curation_reason`), `Pinned`, plus a `curated` flag (`agentops-graph/src/lib.rs`). There is **no** "needs review" state or queue. Milestone 2's import review queue needs a new state or kind.

**AGENTS.md.** `agentops-agents-md` generates it, and `init_agents_md` (MCP, Full mode) and `agentops install` fully overwrite it, with no preserved hand-edit section (gotcha 54112). `connect` reuses an existing `AGENTS.md`, and Ruler copies it into each tool's file: on 2026-10-14, `connect --remote` regenerated `CLAUDE.md` through Ruler. Lines added by hand to `CLAUDE.md` are lost on the next distribution, and lines in `AGENTS.md` are lost on the next `install`. **Subagents don't load either file** (spike).

## 5. The MEMORY.md files to import

A read-only inventory taken 2026-10-14 (`find ~/.claude -name MEMORY.md`). Nothing was moved or changed. 22 files and 301 index entries in total. Entry counts are lines starting with `- `.

| Project directory under `~/.claude/projects/` | Bytes | Entries |
| --- | --- | --- |
| `-Users-jesusglez-Repos-personal-ai-fitness-app` | 9357 | 85 |
| `-Users-jesusglez-Repos-personal-agentops` | 10335 | 48 |
| `-Users-jesusglez-Repos-personal-book-project-ops` | 10337 | 40 |
| `-Users-jesusglez-Repos-semantic-ztrack-v2` | 3803 | 34 |
| `-Users-jesusglez-Repos-DDS-praxis-online-booking` | 2730 | 16 |
| `-Users-jesusglez` (user level) | 2013 | 15 |
| `-Users-jesusglez-Repos-DDS` | 2577 | 15 |
| `-Users-jesusglez-Repos-DDS-concierge-notifications-system` | 1391 | 7 |
| `-Users-jesusglez-Repos-DDS-notifications-front-end` | 848 | 7 |
| `-Users-jesusglez-Repos-impact-ignite-currentyachts` | 681 | 6 |
| `-Users-jesusglez-Repos-transactly-connect-panopticon` | 714 | 5 |
| `-Users-jesusglez-Repos-DDS-voice-agent` | 767 | 4 |
| `-Users-jesusglez-Repos-transactly` | 509 | 3 |
| `-Users-jesusglez-Repos-personal-job-hunter` | 363 | 3 |
| `-Users-jesusglez-Vaults-dev-knowledge-base` | 449 | 3 |
| `-Users-jesusglez-Repos-semantic-ceeav-frontend` | 510 | 3 |
| `-Users-jesusglez-Repos-personal-interview` | 251 | 2 |
| `-Users-jesusglez-Repos-semantic-ceeav-backend` | 144 | 1 |
| `-Users-jesusglez-Repos-DDS-praxis-data-service` | 252 | 1 |
| `-Users-jesusglez-Repos-transactly-connect-next-v2` | 94 | 1 |
| `-Users-jesusglez-Repos-DDS-Concierge` | 198 | 1 |
| `-Users-jesusglez-Repos-tyna-tyna-ai-agent` | 274 | 1 |

Most files point at one `.md` file per entry in the same directory. `ai-fitness-app` keeps all 85 entries inline in one file.

**Representative entries.** Quoted for this repo's file only. This document is in a public repository, and the other files cover client and personal projects, so their entries aren't quoted here. Milestone 2 should sample them privately.

- `[Docbrain-first pivot, now complete](project_vnext_docbrain_first.md) — sequencing change 2026-08-09, docbrain pass finished 2026-08-10`
- `[Prefer the app's own credential flow over pasted secrets when live-testing](feedback_prefer_app_native_credential_flows.md) — confirmed 2026-08-11 after user pushback twice, drove a real architecture fix`
- `[Verify visual/CSS fixes live before claiming done](feedback_verify_before_claiming_a_fix_is_done.md) — corrected 2026-08-13 after wrongly claiming a Tailwind width override worked when tailwind-merge/specificity silently blocked it`
- `[Dashboard unification + connect.sh shipped](project_dashboard_unification_and_connect_sh_shipped.md) — 2026-08-26/27, live-verified end-to-end on thedamnserver; committed 04d5de9/c2743d5/d36cd82, not pushed. Next session: migrate local data to the server.`
- `[Never summarize a code change you didn't verify you made](feedback_never_summarize_a_code_change_you_didnt_verify_you_made.md) — corrected 2026-09-28, claimed a login-link edit was done when no Edit call had happened`

**Rough split. This is a guess**, based on Claude Code's own file-name prefixes, which only 122 of the 301 entries carry:

| Kind | Prefix | Count |
| --- | --- | --- |
| Rules (procedural) | `feedback_` | 26 |
| Past events (episodic) | `project_` | 77 (many also carry durable facts, such as commit hashes and decisions) |
| Facts (semantic) | `reference_`, `user_` | 19 |
| Unclassified | none (inline or unprefixed) | 179 |

For this repo's file: 15 feedback, 28 project, 4 reference, 1 user.

## 6. Open issues

New findings, also in `findings.md`: F50 (hooks never ran here, resolved), F51 (hooks wrote to an orphaned graph, resolved), F52 (tags dropped at ingest, resolved), F53 (recall noise, partly resolved), F54 (Ruler `.gitignore` shrink, open).

Other debt:

- **Recall noise.** One large note matches many prompts on one or two words (F53). Per-keyword IDF weighting is the next step; it belongs with milestone 3's cued recall.
- **Version dependence.** Item 6 depends on `tool_input.plan` and the `ExitPlanMode` deny behaving as on Claude Code 2.1.286. It fails open if the field disappears. Re-check on upgrade.
- **Hooks aren't committed.** This repo's `.claude/settings.json` now carries the hooks, but that change isn't committed. The file is tracked and the repo is public, so committing it would run `agentops hook-capture` for anyone who clones with the CLI installed. The author decides.
- **MEMORY.md is still loaded.** Claude Code still loads it, and the redirect only blocks writes in repos with hooks installed. `autoMemoryEnabled: false` / `CLAUDE_CODE_DISABLE_AUTO_MEMORY` exist in the 2.1.286 binary (**unverified**, not in the docs) and are the switch for after the import.
- **Unmeasured cost.** The council member runs on `model: haiku` with 19 named MCP tools. The cost of the five-member council (gotcha 53946) isn't measured.

Milestone 2 must not break:

- the `user_prompt` / `user_prompt:plan` tool names on `user_intent` events (episode detection and `last_prompt`);
- the `recall_injected` description format `ids 1,2,3`;
- `tags_of` reading only the last line;
- the redirect reason telling agents to tag `memory-kind:semantic|episodic|procedural`. The importer should use the same tags, so imported and agent-written memories route the same way.

Questions for the milestone 2 spec:

1. Where does an entry the importer can't classify go? There is no review state today (section 4). Is it a new `NodeProminence` value, a tag such as `needs-review`, or a dashboard view?
2. Should project MEMORY.md files from other repos (client work) be imported into their own connections, and do those repos have AgentOps connections at all?
3. Should `feedback_` entries become pinned notes now, or wait for the deferred rules milestone?
4. After import, is auto-memory switched off with `autoMemoryEnabled: false` in user settings, or per project?
