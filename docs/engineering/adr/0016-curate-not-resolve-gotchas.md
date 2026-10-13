# 0016. Curate gotchas instead of resolving them

Survey candidate R8. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-12 (`1e56b2a`).

## Context

A first pass modeled gotchas as bugs with `Open`, `Resolved` and `Dismissed` states. A gotcha is permanent knowledge an agent must keep seeing, so it does not reintroduce a solved problem.

## Decision

Replace review status with curation: `curated`, `prominence` (`Full`, `Reduced`, later `Pinned`) and a `curation_reason` required for `Reduced`. No delete, dismiss or hide action exists anywhere. Every agent-facing surface ranks a `Reduced` note ×0.1 and shows the reason.

## Alternatives considered

- A bug-tracker model. Rejected and removed in the same session.

## Consequences

- Dampening applies to sort keys only, never to displayed scores.
- Curation is REST-only; no MCP tool changes it.
- Stale gotchas in `findings.md` get "Reduce prominence", never deletion.

## Evidence

- Decision "Gotchas are curated, never resolved/dismissed -- don't reintroduce a bug-tracker model".
