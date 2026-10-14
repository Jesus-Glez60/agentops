# 0043. Remove the billing role and dead capabilities

Amends ADR 0022. Follows `findings.md` F12 and F13; the author decided on 2026-10-13.

## Status

Accepted, 2026-10-13.

## Context

The permissions matrix still listed `team.billing` and a `billing` role, leftovers of the abandoned paid tier (ADR 0001), and `gotchas.resolve`, which contradicts curating gotchas instead of resolving them (ADR 0016). No route checked either capability.

## Decision

Remove both capabilities and the `billing` role. `migrate_drop_billing_role` (run when the teams store opens) moves billing members and pending invites to `viewer`, re-labels custom roles cloned from `billing` as cloned from `viewer`, strips the two keys from custom roles, and prints what it changed.

## Alternatives considered

- Keeping the role as a no-access role. Not chosen.

## Consequences

- Fixed roles are now admin, member and viewer.
- A former billing member gains viewer access (read-only code and docs), which billing did not have.

## Evidence

- `findings.md` F12, F13.
- `migrate_drop_billing_role`, `ROLES`, `PERMISSIONS_MATRIX` (`agentops-core/crates/agentops-teams/src/lib.rs`).
