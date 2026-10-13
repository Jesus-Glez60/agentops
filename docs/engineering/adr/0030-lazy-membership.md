# 0030. Create membership rows lazily on first touch

Survey candidate S1 (follow-up). Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

Single-user tenants existed before team management. Signup lives in `agentops-accounts`, which does not know `agentops-teams`.

## Decision

`ensure_membership` runs before team and capability checks. It creates the organization row if missing and adds the caller as `admin` if they have no membership. It is an idempotent upsert.

## Alternatives considered

- Create memberships at signup. Rejected: it would thread `TeamStore` into the identity code path.
- A data migration for old tenants. Not needed; first touch heals them.

## Consequences

- Every capability consumer must call it first. A fresh signup was refused on `/repos` until it did (gotcha 53997).

## Evidence

- Decision "New tenants/orgs get membership row lazily via first-touch backfill, not at signup".
