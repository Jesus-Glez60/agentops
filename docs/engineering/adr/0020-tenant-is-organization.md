# 0020. Use the tenant id as the organization id

Survey candidate S1. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

`tenant` was already the isolation key everywhere: docbrain, the code graph, repo access, integrations.

## Decision

Reuse `tenant` as the organization id. Relax `users.tenant` from unique to shared. Organization data lives in a new `agentops-teams` crate and file.

## Alternatives considered

- A new `organizations.id`. Rejected: every consumer already treats `tenant` as an opaque sharding string, so reuse needed no isolation changes.

## Consequences

- Members share their org's connections, docbrain data and credentials automatically.
- `users` and `memberships` are in different files and are merged in Rust.

## Evidence

- Decision "Team model reuses tenant as organization ID, relaxes users.tenant uniqueness".
- Commit `1ec773a`.
