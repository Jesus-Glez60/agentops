# Questions for the author

Questions the code cannot answer: where it shows *what* but not *why*, or where two parts disagree. Answers here are the only allowed source of rationale that is not already in code or a recorded note.

Format: question, what the code already shows, and what the answer unblocks. Answered items move to the bottom with the answer and date.

## Open

### Q1. Is embedding the full server in the CLI binary intended long-term?

- Code shows: `agentops-cli` depends on `agentops-server`. `agentops_server::run` is documented as "the one entry point both this crate's own binary and `agentops-cli`'s `serve-api` subcommand call" (`agentops-core/crates/agentops-server/src/lib.rs`). So the coupling is deliberate, not accidental.
- Still unknown: why ship one `agentops` binary that contains the full multi-tenant server (all heavy crates included) instead of telling users to run `agentops-server`? Is binary size or compile time an accepted cost?
- Unblocks: ADR A5, `01-architecture.md` binaries section.

### Q5b. Why is the access mode per-org on hosted `/mcp` but per-process on REST `/tools`?

- Follow-up to answered A-5 (why `advisor` is the default).
- Code shows: hosted `/mcp` reads `organizations.mcp_access_mode` per call (added `ee9b7e9`, 2026-08-31). REST `/tools` on the same `agentops-server` process reads env `AGENTOPS_ACCESS_MODE` once.
- Ask: is the mismatch intended? Why per-org rather than per-API-key?
- Unblocks: ADR M3, `09-auth-teams-security.md`.

### Q6. Which write tools are meant to be available in Advisor mode?

- Code shows these writes available in Advisor:
  - `add_note`, with a stated reason (`agentops-core/crates/agentops-mcp/src/tools.rs:96-102`).
  - `ingest_notes` and `persist_blueprint_doc`, with no reason given.
  - All docbrain writes (`register_library`, `scrape_library`, `sync_changelogs`), because docbrain has no access mode.
  - Stdio heavy tools, including the long-running `consolidate_model`.
  - Hosted `unregister_repo`, which is annotated `destructive_hint: true`.
- Code already explains two groups:
  - docbrain and heavy tools: "retrofitting a shared capability system into two more crates is out of scope for this merge, whose goal is removing tiering" (`agentops-core/crates/agentops-mcp-server/src/lib.rs`, module doc).
  - heavy tools: "every tool this server has is read/index-only (no code modification), so there's no write-capable variant to structurally gate" (`agentops-core/crates/agentops-heavy-mcp/src/lib.rs`).
- `ingest_notes` also has a stated reason: its `ToolSpec` comment says "same reasoning as `add_note`".
- Still unexplained: `persist_blueprint_doc`, hosted `unregister_repo`, and whether "deferred, not designed" is still the intended state for docbrain writes.
- Related: both note tools accept a `notes_path` that hosted `/mcp` does not sanitize (`findings.md` F31). That is a bug whatever the answer here.
- Ask: is each of these intended? If yes, why? If not, they go to `findings.md`.
- Unblocks: `07-mcp-and-rest-contracts.md`, `09-auth-teams-security.md`.

### Q7. Is the consolidation no-regression gate tuned as intended?

- Brief's question ("what does the eval measure, and what counts as a regression?"). Code now answers most of it:
  - Embedding consolidation (`agentops-core/crates/agentops-embeddings-train/src/lib.rs`): recall@5 (`RECALL_K = 5`) on a 20% held-out split (`HELD_OUT_FRACTION = 0.2`) of replay pairs drawn from `Affects`/`References` edges. Promoted iff `candidate_recall >= baseline_recall`.
  - LoRA consolidation (`agentops-core/crates/agentops-heavy-consolidate/src/eval.rs`, `src/lib.rs`): mean cosine similarity between a greedy generation and the curated note text, scored by the frozen base embedder so the eval cannot reward drift toward the trained projection. Promoted iff `candidate_score >= baseline_score`. The module doc says the gate "reduces regression risk, it does not guarantee quality".
  - A regression is therefore any candidate scoring strictly below the active model on the held-out split.
- Still unknown: is promoting on a tie (`>=`) intended, given it lets an unchanged-quality candidate replace the active one? Is there a minimum improvement margin you wanted?
- Unblocks: `06-notes-and-consolidation.md`, ADR R3.

### Q8. Where does CI run?

- Code shows: `.github/workflows/` has only `release.yml`. Nothing in the repo describes a test gate.
- Ask: where do tests run before merge or deploy, and should `12-development-workflow.md` document it (or is it private infrastructure)?
- Unblocks: `12-development-workflow.md`, `11-deployment-and-operations.md`, ADR T3.

### Q9. Should `repo-map.md` be committed, or the README link changed?

- See `findings.md` F2. The docs need to know whether to link `repo-map.md` at all.
- Unblocks: `00-index.md`, `01-architecture.md`.

### Q10. Why SQLite by default, and Postgres for the code graph only?

- Brief's question. Code shows: SQLite core from `7a393e7` (2026-07-28); Postgres `GraphStore` from `0385f5e` (2026-07-28), the paid heavy tier (see A-2). docbrain, accounts and teams stay on SQLite even when `AGENTOPS_DATABASE_URL` is set (to confirm in Phase 3).
- Ask: was Postgres only ever meant for the hosted SaaS scale, and is that still the reason now that the suite is open source?
- Unblocks: ADR A3, `03-data-model.md`.

### Q11. Why one unified MCP server instead of the earlier three?

- Brief's question. Decision note "Backend merge: 3 REST APIs + 3 stdio MCP servers collapsed into agentops-server + agentops-mcp-server" exists. Commit `218c3fc` says "no price gate, single REST/MCP process, minimal env vars". With A-2, the price gate is the likely driver.
- Decision note text (read in Phase 5): the split "existed only because `agentops-heavy` was a separate Cargo workspace licensed `LicenseRef-Commercial`", and there was never a runtime licence check. ADR 0002 records this.
- Ask: is that the whole reason, or did client config friction (one MCP entry instead of three) also matter?
- Unblocks: ADR A1.

### Q12. Should node recency use the 30-day `Affects` half-life?

- Code shows: hybrid ranking multiplies each hit by `effective_weight(1.0, age_days(last_touched_at))` (`recency_multiplier`, `agentops-core/crates/agentops-retrieval/src/lib.rs`). `effective_weight` defaults to `AFFECTS_EDGE_HALF_LIFE_DAYS = 30`. A symbol not edited for 60 days ranks at 0.25× a freshly edited one with the same fused score.
- Postgres never populates `last_touched_at`, so the same query ranks differently on SQLite and Postgres.
- Ask: is penalizing stable, unchanged code intended? Was a separate node half-life considered?
- Unblocks: `05-retrieval.md`, ADR R3.

### Q13. Do the LoRA consolidation deferrals still stand?

- Code shows: `consolidate_model` promotes a LoRA adapter that nothing uses for generation (`findings.md` F29). The shipping decision deferred three follow-ups: a local-first `explain_symbol`, a consolidation-status endpoint, and an `agentops consolidate --model` command.
- The stated blocker was the paid-tier crate boundary, which the open-core merge removed.
- Ask: are these still planned, dropped, or should `consolidate_model` be hidden until a consumer exists?
- Unblocks: `06-notes-and-consolidation.md`, ADR R3.

## Answered

### A-1. Should AgentOps's own docs be registered in docbrain?

- Answer (author, 2026-10-12): no. Read local docs directly; no docbrain registration or scraping for AgentOps. The brief's audience note about ingesting via `scrape_library` with `slug: agentops` is out of scope.

### A-2. What does "heavy" mean today? (was Q2)

- Answer (author, 2026-10-12): nothing current. It is a remnant of a plan to sell AgentOps as SaaS: a paid "heavy" suite and a free "light" suite. The author chose the open-source route instead, and the heavy/light split no longer means anything.
- Consequences for the docs:
  - The glossary defines "heavy" as a historical name only.
  - Layer and crate descriptions use functional names; `heavy` appears only as part of crate names.
  - The leftover naming is logged in `findings.md` F11.
  - ADR A2 records the abandoned SaaS tier; ADR A1 records the merge that removed it.

### A-3. Full list of Decision notes (was Q3)

- Answer (author, 2026-10-12): all decisions were migrated into the hosted AgentOps graph. Early work used a local DB; check it for information.
- Done: `.context/graph.db` (local graph, last written 2026-10-05) holds 50 `decision`, 91 `gotcha` and 10 `note` nodes for repo `agentops`. All 50 decisions were listed; `survey/adr-candidates.md` now uses that list as its primary source. All 50 show `last_touched_at` 2026-09-21, which is likely the migration date, not the original decision date; git history dates them instead.

### A-4. Layer names for `01-architecture.md` (was Q4)

- Answer (author, 2026-10-12): approved, as follows. The "heavy" row is renamed per A-2.

| Layer | Crates |
| --- | --- |
| Foundations | `security`, `sqlite-support`, `embeddings`, `repo-access`, `agents-md`, `ruler-bridge`, `lsp-client`, `github-app`, `integration-modules` |
| Code graph and storage | `graph`, `graph-pg`, `store-open` |
| Indexing | `scanner`, `docgen` |
| Repo registry | `manifest` |
| Knowledge and retrieval | `retrieval`, `notes`, `llm`, `embeddings-train` |
| Integrations | `linear`, `integrations` |
| docbrain | `docbrain-graph`, `docbrain-ingest`, `docbrain-mcp`, `docbrain-api` |
| Semantic index and consolidation | `heavy-embeddings`, `heavy-consolidate`, `heavy-mcp` |
| Accounts and teams | `accounts`, `teams` |
| Use cases and transports | `mcp`, `api`, `heavy-api` |
| Binaries | `server` (also a library), `mcp-server`, `cli` |

### A-5. Why is `advisor` the default? (was Q5, first half)

- Answer (author, 2026-10-12): the MCP client can be any code editor or agent, from vim/nvim to VS Code and Cursor. The server cannot know or trust which one is calling, so the safe default applies.
- Matches the code comment on `AccessMode::Advisor`: "the safe default for an untrusted or read-only integration" (`agentops-core/crates/agentops-mcp/src/tools.rs`).
- The brief's "why hide write tools rather than reject calls" has a false premise: Full-only tools are hidden **and** refused (`call_tool`'s backstop).
- The second half of Q5 stays open as Q5b.
