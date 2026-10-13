# AgentOps engineering docs — Claude Code brief

Oct 12, 2026 · @Jesus Gonzalez

## How to use this brief

This is the spec you hand Claude Code so it can retro-document AgentOps. You did the engineering; the agent reads the code and drafts, and you supply the reasons behind decisions.

1. Open Claude Code at the root of your local `agentops` checkout, on a fresh branch (`docs/engineering`).
2. Make sure AgentOps is connected to itself: `agentops install --with-embeddings`, then `agentops connect --agents claude`. Keep the default `advisor` (read-only) access mode for the research phases.
3. Run `agentops docgen` so `repo-map.md` exists locally. The README links it, but it is not committed.
4. Paste the kickoff prompt from the last section. The agent works in the phases below and stops at each checkpoint for you.
5. Answer its "ask the author" questions in the session. Those answers are the part no tool can recover from the code.

The agent writes Markdown files. You then import them into a shareable doc here; see Deliverables for the one-file-per-document layout that makes that easy.

## Goal, audience, non-goals

**Goal:** a complete engineering documentation set that lets a new maintainer, or an AI agent with no prior session, understand how AgentOps is built and why, and change it safely.

**Audience:**

- **Contributors and maintainers:** you in six months, and future collaborators once PRs reopen. They need structure, contracts, invariants and the reasoning behind decisions.
- **AI agents:** sessions that will ingest these docs through docbrain (`scrape_library` with `slug: agentops`) or read them directly. They need stable headings, exact identifiers, and one fact per place.

**Non-goals:**

- No user or self-hoster guide. The README already covers install and connect; link to it instead of repeating it.
- No marketing copy. `docs/marketing-brief.md` stays separate.
- No code changes. If the agent finds a bug or a README inaccuracy, it lists it in a findings file; it does not fix it.
- No regenerated API reference. `docs/openapi.yaml` is the REST source of truth; docs link to it rather than restating endpoints.

## Repo facts to anchor on

The workspace has 36 crates (32 under `agentops-core/crates`, 4 under `docbrain-core/crates`), three binaries, and two frontends. The grouping below comes from each crate's internal dependencies in `Cargo.toml`; the agent should confirm it against the graph, not trust it blindly.

| Layer | Crates | Notes |
| --- | --- | --- |
| Foundations (no internal deps) | `security`, `sqlite-support`, `embeddings`, `repo-access`, `agents-md`, `ruler-bridge`, `lsp-client`, `github-app`, `integration-modules` | Leaf crates; everything else builds on these |
| Graph and storage | `graph`, `graph-pg`, `store-open` | SQLite default, Postgres (pgvector) via `graph-pg`; `store-open` picks the backend |
| Indexing | `scanner`, `docgen`, `manifest` | Tree-sitter scanning, `repo-map.md` generation, local repo manifest |
| Knowledge and retrieval | `retrieval`, `notes`, `llm`, `embeddings-train`, `linear` | Hybrid search, gotchas/decisions, LLM features, local consolidation |
| docbrain | `docbrain-graph`, `docbrain-ingest`, `docbrain-mcp`, `docbrain-api` | Same pattern for dependency docs; reuses `embeddings` and `sqlite-support` |
| Hosted / multi-tenant | `accounts`, `teams`, `integrations`, `heavy-embeddings`, `heavy-consolidate`, `heavy-mcp`, `heavy-api` | Orgs, roles, GitHub App, BGE-M3 + Qdrant |
| Transports | `mcp`, `api` | Use-case layer exposed over MCP and REST |
| Binaries | `server`, `mcp-server`, `cli` | `agentops-server`, `agentops-mcp-server`, `agentops` |

Other anchors:

- **Frontends:** `apps/web` (Next.js, has its own `CLAUDE.md`) and `apps/marketing` (Astro).
- **Deploy:** Dockerfile, `docker-compose.yml`, `deploy/k8s/`, `ecosystem.config.js`, `install.sh`, `Caddyfile.example`.
- **Existing docs:** `docs/openapi.yaml`, `docs/vault-protocol.md`, `llms.txt`, `llms-full.txt`, `CONTRIBUTING.md`, `SECURITY.md`.
- **Agent tooling in the repo:** `.claude/hooks/block-local-notes.sh` and `.claude/settings.json`; the vault protocol also references the `save-note` and `wrap` skills and a `vault-archivist` subagent.

Things the agent should flag, not guess:

- `agentops-cli` depends on `agentops-server`. Is the CLI meant to embed the full server for `serve-api`, or is that coupling accidental?
- The "heavy" crates are not only heavy embeddings: `heavy-api` pulls in accounts, teams and the GitHub App. What does "heavy" mean as a boundary?
- `repo-map.md` is gitignored, yet the README links it as the architecture map.

## Deliverables

The agent writes one Markdown file per document under `docs/engineering/` on the docs branch. Nothing is committed until you decide; you import the files into a shareable doc here, one tab per file. Committing them later is worth considering, since docbrain ingests docs from the repo.

| File | Covers | Must include |
| --- | --- | --- |
| `00-index.md` | Entry point | One-line summary per doc, reading order for a new maintainer, glossary (node, edge, gotcha, decision, consolidation, advisor/full, heavy) |
| `01-architecture.md` | System overview | Component diagram (Mermaid), the layer table from this brief verified, the three binaries and what each wires together, one traced request per transport |
| `02-crate-reference.md` | Every crate | Per crate: purpose, public entry points, internal deps, invariants, the tests that cover it |
| `03-data-model.md` | Storage | Node and edge types, SQLite vs Postgres schemas, where embeddings live, migrations, `migrate-graph` |
| `04-indexing-pipeline.md` | Scan to graph | Scan, parse, link, embed, docgen; incremental rescans, `watch`, push-triggered reindex, the optional rust-analyzer pass; sequence diagram |
| `05-retrieval.md` | Search | Dense, BM25 and exact-name signals, how they are fused and weighted, PageRank expansion, the BGE-M3 + Qdrant path, tuning knobs |
| `06-notes-and-consolidation.md` | Tribal knowledge | Symbol matching, stale flagging, the vault protocol, local fine-tuning and its no-regression gate, `librarian` |
| `07-mcp-and-rest-contracts.md` | Interfaces | Full MCP tool inventory (name, inputs, output, read or write, which access mode exposes it); REST parity rules; link to `openapi.yaml` instead of restating it |
| `08-docbrain.md` | Dependency docs | Registration, version-pinned scraping, `llms.txt` handling, how it shares the embeddings layer |
| `09-auth-teams-security.md` | Trust boundaries | Accounts, roles, invites, API keys, device-flow login, the secrets master key, audit log, advisor vs full enforcement |
| `10-frontends.md` | Web and marketing | `apps/web` structure, how it calls the API, `/suite` base path, `apps/marketing` |
| `11-deployment-and-operations.md` | Running it | Env var reference (link `.env.example`), storage choices, each deploy method, upgrades, failure runbook (scan stuck, embeddings missing, webhook not firing) |
| `12-development-workflow.md` | Working on it | Build and test commands, CI in `.github/`, `deny.toml`, the Claude Code setup (hooks, skills, review agents) |
| `13-agent-integration.md` | Coding-tool side | `connect`, Ruler, AGENTS.md generation, hooks and command compression, usage tracking |
| `adr/NNNN-<slug>.md` | Decisions | One record per significant decision: context, decision, alternatives, consequences, status |
| `findings.md` | Out of scope | Bugs, README inaccuracies, dead code, naming problems found along the way; never fixed in this task |

## Method

Every claim in the docs must trace to code, an existing note, or the author. The graph finds things fast; the source file is the proof.

**Research with AgentOps first:**

1. Pull existing gotchas and decisions for the area before reading code. They are the seed for ADRs and invariants.
2. Use hybrid search with graph expansion to find the symbols involved, then open the actual files.
3. Use `repo-map.md` for the overall shape only. It is generated and may lag the code.
4. Use `changelog` and git history to date decisions and to see what was replaced.

**Verify against code:**

- Every crate, symbol, env var, CLI flag and MCP tool name appears exactly as in the source, in backticks, with a file path.
- Counts are measured, not copied. The README says "about 40" MCP tools; the inventory states the real number.
- Diagrams are Mermaid, so they stay editable and diffable.

**Ask the author:**

The agent keeps a running `questions.md`. A question goes there whenever the code shows *what* but not *why*, or two parts of the code disagree. It never invents rationale. Examples of the kind expected:

- Why SQLite as the default, and Postgres only for the code graph?
- Why one unified MCP server instead of the earlier three?
- Why is `advisor` the default, and why hide write tools rather than reject calls?
- What does the consolidation eval measure, and what counts as a regression?

**Writing rules (for both audiences):**

- Each doc opens with a two-sentence summary of what it covers and who needs it.
- One fact lives in one place; other docs link to it.
- Headings are stable nouns (`## Retrieval fusion`), so agents can cite them.
- Sentences under 25 words; no filler, no marketing tone.
- Mark anything unverified as `> Unverified:` with what would confirm it.

## Phases and checkpoints

The work runs in five phases. The agent stops at each **checkpoint** and waits for you; it never writes a full document before you have approved its outline.

1. **Survey** (advisor mode). Verify the layer table, build the MCP tool inventory, list candidate ADRs from existing decisions and git history, and start `questions.md`. **Checkpoint:** you correct the layers and answer the first batch of questions.
2. **Outlines.** One outline per document: headings plus one line each on what goes there and its source. **Checkpoint:** you approve or cut sections.
3. **Foundation docs:** `01-architecture`, `02-crate-reference`, `03-data-model`. Everything else links to these, so they go first. **Checkpoint:** you review them before the rest builds on them.
4. **Subsystem docs:** `04` to `13`, one per session if context gets tight. Each ends with its open questions. **Checkpoint** after each, or in pairs.
5. **ADRs, index, findings.** Write the ADRs from your answers, then `00-index` last, since it summarizes the rest. **Checkpoint:** final review.

After you approve the set, record the main decisions back into AgentOps with `add_note` (switch to `full` mode for that step only), following `docs/vault-protocol.md`. That closes the loop: the next session finds these decisions in the graph.

## Done when

- [ ] Every file in the Deliverables table exists and opens with its two-sentence summary.
- [ ] All 36 crates appear in `02-crate-reference.md`; a script comparing it against the workspace members in `Cargo.toml` finds none missing.
- [ ] The MCP inventory matches the tools the server actually registers, counted from source, with access mode per tool.
- [ ] Every identifier in backticks exists in the repo (spot-check 20 with grep; zero misses).
- [ ] Every Mermaid diagram renders.
- [ ] No rationale appears that is not traceable to code, an existing note, or an answer in `questions.md`.
- [ ] `questions.md` has no unanswered items, or the remaining ones are marked as open in the relevant doc.
- [ ] A fresh Claude Code session, given only `docs/engineering/`, can correctly answer: where a new MCP tool is added, how a search query is scored, and what happens on a GitHub push.
- [ ] No source file outside `docs/engineering/` was modified.

## Kickoff prompt

Claude Code cannot read this doc, so export it as Markdown and save it as `docs/engineering/BRIEF.md` first. Then paste:

```markdown
Read docs/engineering/BRIEF.md in full. It is the spec for this task:
writing the engineering documentation set for AgentOps, retroactively.

Rules:
- Use the agentops MCP tools (search, gotchas/decisions, changelog) to
  research, but verify every claim in the source before writing it.
- Write only under docs/engineering/. Do not modify any other file.
- Never invent the reason behind a decision. Add it to
  docs/engineering/questions.md and ask me.
- Put bugs and README inaccuracies in docs/engineering/findings.md.
  Do not fix them.
- Work in the phases in the brief and stop at every checkpoint.

Start with Phase 1 (Survey). Before doing anything else, give me a short
plan for the survey and tell me anything in the brief that is ambiguous
or contradicts what you see in the repo.
```

The last line is the ambiguity check from the spec exercise: the agent reviews the brief against the real repo before it starts.
