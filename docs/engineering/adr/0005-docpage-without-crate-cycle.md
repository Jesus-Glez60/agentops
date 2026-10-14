# 0005. Structured DocPage, LLM-free docgen, opaque persistence

Survey candidate A7. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-13 (`0b0f553`). Amended by ADR 0044 (module labels reused while the file set is unchanged).

## Context

The Documentation Viewer needed a sectioned `DocPage` with LLM-labelled core modules and a freshness signal. `agentops-docgen` depends on `agentops-graph`, so the graph crate cannot know `DocPage`.

## Decision

1. `build_doc_page` stays LLM-free. It takes precomputed `ModuleLabel`s as data and falls back to a directory heuristic when none are given.
2. `ModuleLabel` lives in `agentops-graph`, which both `agentops-docgen` and `agentops-llm` already depend on.
3. `GraphStore::save_doc_page`/`get_doc_page` take and return a JSON string, never a typed `DocPage`.
4. `agentops-mcp`, which depends on all three, orchestrates the steps in `scan::persist`. Failures there are logged and never fail the scan.

## Alternatives considered

- Give docgen a network dependency. Rejected: it contradicts docgen's role as a pure renderer.
- A typed `&DocPage` on the trait. Rejected: it creates a crate cycle.

## Consequences

- The REST handler serves the stored JSON as-is; nothing deserializes `DocPage`.
- Because orchestration runs in `scan::persist`, every scan with an API key makes a paid LLM call (`findings.md` F24).

## Evidence

- Decision "Documentation Viewer backend: structured DocPage, LLM-free docgen, opaque-string persistence to avoid a crate cycle".
- Commit `0b0f553`.
