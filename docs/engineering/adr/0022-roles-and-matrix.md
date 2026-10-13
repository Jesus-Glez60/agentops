# 0022. Fixed plus custom roles, one permissions matrix

Survey candidate S3. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

Roles needed to be extensible per org, and the UI and the routes must agree on what each role can do.

## Decision

Four fixed roles (`admin`, `member`, `viewer`, `billing`) plus per-org custom roles cloned from a fixed one, with keys `custom:{id}`. `TeamStore::is_valid_role` checks both. `PERMISSIONS_MATRIX` drives both the Roles & Permissions screen and `has_capability`.

## Alternatives considered

- Fixed roles only. Replaced: every new role needed a code change.
- Separate display and enforcement definitions. Rejected: they "inevitably diverge".

## Consequences

- Capability keys are constants, so a typo is a compile error.
- Two capabilities are defined but never enforced (`findings.md` F12, F13). MCP and curation sit outside the matrix (`findings.md` F38).

## Evidence

- Decisions "Roles are fixed + custom, validated at both hardcoded enum and database table" and "Permissions matrix is the single source of truth for both display and enforcement".
