# 0038. One `agentops` binary that embeds the server

Survey candidate A5. Rationale comes from the author's answer A-6 in `questions.md`.

## Status

Accepted. Rationale recorded 2026-10-13.

## Context

`agentops-cli` depends on `agentops-server`, and its `serve-api` subcommand calls `agentops_server::run`, so the CLI binary contains the full multi-tenant server.

## Decision

Ship one `agentops` binary. A single install matters more than binary size or compile time.

## Alternatives considered

- Telling users to install and run `agentops-server` separately. Not chosen: two installs for one product.

## Consequences

- The CLI binary carries every server crate, so it is larger and slower to build.
- The npm package (`npx agentops-cli`) ships the same full binary.

## Evidence

- Author answer A-6, `questions.md`.
- Doc comment on `agentops_server::run` (`agentops-core/crates/agentops-server/src/lib.rs`).
