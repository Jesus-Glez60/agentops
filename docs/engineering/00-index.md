# AgentOps engineering docs

The entry point to AgentOps's engineering documentation: which document answers which question, and in what order to read them. Every claim in these docs was checked against the source at commit `8565155` or later; reasons for decisions come only from recorded notes, code comments or the author's answers.

## Documents

| Doc | Answers |
| --- | --- |
| [01-architecture.md](01-architecture.md) | What the system is, its layers and binaries, how a request flows, and the invariants that cut across crates. |
| [02-crate-reference.md](02-crate-reference.md) | What each of the 36 crates does, its entry points, dependencies, invariants and tests. Split by group under [02-crates/](02-crates/). |
| [03-data-model.md](03-data-model.md) | Node and edge kinds, identity, every table on SQLite and Postgres, migrations, deletion. |
| [04-indexing-pipeline.md](04-indexing-pipeline.md) | How a repo becomes a graph: walking, parsing, persisting, note linking, embeddings, hosted indexing, webhooks. |
| [05-retrieval.md](05-retrieval.md) | How search ranks results: modes, fusion, graph expansion, gist-then-detail, prominence, the Qdrant path, tuning knobs. |
| [06-notes-and-consolidation.md](06-notes-and-consolidation.md) | How gotchas, decisions and notes are written, matched, curated and kept fresh; local model consolidation. |
| [07-mcp-and-rest-contracts.md](07-mcp-and-rest-contracts.md) | Every MCP tool and surface, access modes, wire protocol, output budgets, REST parity, adding a tool. |
| [08-docbrain.md](08-docbrain.md) | Third-party library docs: discovery, scraping, chunking, retrieval, changelogs, tenancy. |
| [09-auth-teams-security.md](09-auth-teams-security.md) | Credentials, tenants and roles, secrets, repo access, audit log, hosted exposure checklist. |
| [10-frontends.md](10-frontends.md) | The Next.js web app and the Astro marketing site: structure, backend proxy, UI rules, tests. |
| [11-deployment-and-operations.md](11-deployment-and-operations.md) | Configuration, storage, deploy methods, upgrades, health, and the runbook. |
| [12-development-workflow.md](12-development-workflow.md) | Building, testing, supply chain, releases, and the repo's own Claude Code setup. |
| [13-agent-integration.md](13-agent-integration.md) | `agentops connect`, supported tools, prompt pack and Ruler, hooks, usage tracking. |
| [adr/](adr/README.md) | 35 architecture decision records, plus candidates waiting for a recorded reason. |
| [findings.md](findings.md) | Bugs, doc inaccuracies, dead code and stale gotchas found while writing these docs. Nothing in it is fixed yet. |
| [questions.md](questions.md) | Questions only the author can answer, and the answers received. |

Supporting material: [survey/](survey/) holds the Phase 1 measurements (layers, MCP inventory, ADR candidates), [outlines.md](outlines.md) the approved outlines, and [scripts/](scripts/) the checks used to verify these docs.

## Reading order

**New maintainer:** 01 → 02 (skim the index, read your group) → 03 → 07 → the subsystem doc you will touch → 12 → the ADRs it links.

**Coding agent starting a task:** 07 → the crate section in 02 → the topic doc. Then check recorded gotchas with `list_gotchas` and `get_symbol` before changing code.

**Operator:** 11 → 09 → `findings.md` items tagged Security.

**Reviewer of a security change:** 09 → 07 ("Hosted path resolution") → `findings.md` F31, F35, F37, F38, F39.

## Glossary

| Term | Meaning |
| --- | --- |
| Node | One entry in the code graph: `File`, `Symbol`, `Gotcha`, `Decision`, `Note`, `Definition`, `DocSection` or `ToolOutput` (`NodeKind` in `agentops-core/crates/agentops-graph/src/lib.rs`). |
| Edge | A typed link between nodes: `DependsOn`, `Documents`, `Affects`, `References`, `Covers` (`EdgeRelation`). |
| Gotcha | A recorded bug, quirk or workaround that agents must keep seeing. Permanent; curated, never resolved. |
| Decision | A recorded choice and its reason. |
| Note | Other recorded knowledge (`NodeKind::Note`; note types `knowledge` and `context`). |
| Definition | An LLM-written explanation of a symbol, from `explain_symbol`. |
| Curation, prominence | How a person ranks a note: `Full`, `Reduced` (×0.1, with a reason) or `Pinned`. |
| Plasticity | Edge weights that grow on reinforcement and decay with age. |
| Consolidation | Training local models from the graph: a projection head for embeddings, a LoRA adapter for explanations. |
| Advisor / Full | MCP access modes. Advisor hides and refuses write tools except knowledge writes; Full allows all. |
| Connection | A repo registered with a hosted server, identified by a connection id. |
| Tenant, organization | The same thing: the isolation key for all hosted data (ADR 0020). |
| Hosted repo name | `<connection_id>--<tenant>`, the code-graph repo key on a hosted server. |
| docbrain | The third-party library documentation subsystem. |
| Heavy | Historical prefix from an abandoned paid tier; carries no meaning now (ADR 0001). |
| Instance key | `AGENTOPS_API_KEY_HASH`, the operator-wide REST key with no user or tenant. |
| Personal API key | A per-user key from the web app or the device flow. |
| Remote marker | `.context/agentops-remote.json`: marks a repo as connected to a server; holds its URL, connection id and key. |
| Prompt pack | The skills, agents and shared text under `agentops-core/prompts/`, distributed by Ruler. |

## Related documentation

- `README.md`: product overview, install and quick start.
- `docs/openapi.yaml` and `docs/index.html`: the REST reference (hand-written; see ADR 0004).
- `docs/vault-protocol.md`: how agents record notes.
- `SECURITY.md`: vulnerability reporting and supply-chain notes.
- `CONTRIBUTING.md`: contribution status and rules.
- `apps/web/CLAUDE.md`, `apps/marketing/README.md`: per-app notes.

`repo-map.md` is generated locally and gitignored, so it is not linked here (`questions.md` Q9).

## Maintaining these docs

- Run the checks in `scripts/` after editing: `check-identifiers.py` (every backticked identifier exists), `check-mermaid.mjs` (diagrams parse), `crate-deps.sh --check` (every crate is documented), `mcp-tools.sh` (tool counts).
- Put a new reason for a decision in an ADR only when it is recorded somewhere; otherwise add a question.
- Put bugs in `findings.md`, not in the topic docs' prose.
