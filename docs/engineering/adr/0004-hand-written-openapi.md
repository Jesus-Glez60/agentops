# 0004. Hand-write the OpenAPI spec

Survey candidate A6. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-25 (`5918ede`).

## Context

The repo had no README, license file or API reference. The REST surface spans three crates.

## Decision

`docs/openapi.yaml` is a hand-written OpenAPI 3.0.3 spec, transcribed from the Rust request and response structs. `docs/index.html` is rendered from it with Redoc. Any route change must update the spec by hand.

The same change made `agentops-server` register `agentops` as a docbrain library on every boot.

## Alternatives considered

- Generate the spec with `utoipa`. Rejected: retrofitting it across three crates (the note cites "the plan's decision to hand-write rather than retrofit").
- OpenAPI 3.1. Not used: `nullable: true` is invalid there (gotcha 54036).

## Consequences

- No compile-time link between handlers and spec, so it drifts. Known gaps are listed in `07-mcp-and-rest-contracts.md`, "OpenAPI spec".
- The self-seed is invisible to hosted orgs (`findings.md` F3); the author decided AgentOps's own docs will not be registered (`questions.md` A-1).

## Evidence

- Decision "README, OpenAPI spec, llms.txt, and docbrain self-seed shipped".
- Commit `5918ede`.
