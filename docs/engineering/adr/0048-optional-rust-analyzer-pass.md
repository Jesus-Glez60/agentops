# 0048. The rust-analyzer pass is opt-in

Survey candidate R7. Rationale comes from the author's answer of 2026-10-14; gotchas 53903 and 53905 describe the pilot.

## Status

Accepted. Rationale recorded 2026-10-14.

## Context

`agentops install --with-lsp` runs rust-analyzer to expand item-level macros and, since `9eb75fd`, to add cross-file `References` edges. Tree-sitter alone covers every other scan.

## Decision

Keep it opt-in. Spawning rust-analyzer adds real latency to every scan, and it needs rust-analyzer installed (`rustup component add rust-analyzer`).

## Alternatives considered

- Running it on every scan of a Rust repo. Not chosen, for the cost and dependency above.

## Consequences

- Default graphs have only same-file `References` edges.
- The pass waits for rust-analyzer's own `$/progress` work before querying (`findings.md` F47).

## Evidence

- Author answer, 2026-10-14; gotchas 53903, 53905.
- `persist_with_lsp` (`agentops-core/crates/agentops-mcp/src/scan.rs`); `agentops-core/crates/agentops-lsp-client`.
