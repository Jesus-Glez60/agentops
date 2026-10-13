# 0011. Return affecting gotchas and decisions from get_symbol

Survey candidate M7. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

Notes attached to a symbol only appeared in the generated `repo-map.md`, not in the tool an agent calls just before touching that symbol.

## Decision

`get_symbol` looks up incoming `Affects` edges and appends "Known gotchas affecting this symbol" and "Decisions affecting this symbol". Later work added staleness and curation markers and weight-based ordering.

## Alternatives considered

- Require a full docgen run first. Rejected: `get_symbol` is the higher-traffic, more targeted moment.

## Consequences

- A gotcha resurfaces exactly when it is relevant.
- Output is capped per note (`budget::cap`).

## Evidence

- Decision "get_symbol surfaces affecting gotchas and decisions directly".
- `tool_get_symbol` in `agentops-core/crates/agentops-mcp/src/tools.rs`.
