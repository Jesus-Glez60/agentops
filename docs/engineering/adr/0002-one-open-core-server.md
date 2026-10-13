# 0002. Merge three servers into one open-core process

Survey candidate A1. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-24 (`218c3fc`).

## Context

Three REST binaries (`agentops-api` :8420, `docbrain-api` :8421, `agentops-heavy-api` :8978) and three stdio MCP servers existed. The split existed only because the heavy crates were a separate, commercially licensed workspace. It was kept out of the MIT/Apache root so the CLI would not absorb commercial code. There was never a runtime license check (decision note, "confirmed by direct audit").

## Decision

Merge into two binaries: `agentops-server` (one Axum process, one port, default 8420) and `agentops-mcp-server` (one stdio process). All crates move into the root workspace under `MIT OR Apache-2.0`.

- Each REST crate keeps its own `build_router()` plus variants without `/health` or `/tools`, because `Router::merge` panics on duplicate routes.
- Route collisions are resolved by renaming: `agentops-api`'s `/repos` → `/scans`, `/search` → `/local-search`; `docbrain-api` is nested under `/docbrain`.
- Tool-name collisions are resolved by renaming in the source crates, not by prefixing: `local_semantic_search`, `list_scans`, `search_docs_indexed`, `index_docs_indexed`.
- `agentops-mcp-server` dispatches by checking each table's own name list in order.

## Alternatives considered

- Keep separate binaries. Their only reason was the license boundary, which the open-source decision removed (ADR 0001).
- Prefix tool names per source table. Not chosen; the note records renaming inside each crate instead.

## Consequences

- One process and one port for operators; one MCP entry for clients.
- `agentops-heavy-api`'s REST `/tools` stays unmounted; its tools are reachable over stdio only.
- A unified REST `/tools` dispatcher and crate renames were left as follow-ups.
- The CLI still ships the pre-merge stdio servers (`findings.md` F17).

## Evidence

- Decision "Backend merge: 3 REST APIs + 3 stdio MCP servers collapsed into agentops-server + agentops-mcp-server".
- Commit `218c3fc` "Merge into one open-core: no price gate, single REST/MCP process, minimal env vars".
- Module doc of `agentops-core/crates/agentops-mcp-server/src/lib.rs`.
- `questions.md` Q11 asks whether anything beyond the licence split drove it.
