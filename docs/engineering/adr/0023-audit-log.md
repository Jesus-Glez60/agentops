# 0023. Insert-only audit log, Owner and Admin only

Survey candidate S4, S8. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

Team actions need a trustworthy record, and seeing every member's actions is a different trust tier from managing members.

## Decision

- `TeamStore` exposes only `record_audit_event` and `list_audit_log`; no code path updates or deletes rows.
- `GET /team/audit-log` is gated by `require_admin`, outside the permissions matrix, so no custom role can be granted it.

## Alternatives considered

- WORM storage or hash chaining. Judged overkill at this stage.
- A matrix row for audit visibility. Rejected to prevent accidental over-delegation.

## Consequences

- Integrity relies on application code; SQLite does not enforce it.
- Org deletion keeps audit rows (ADR 0026).
- Only team actions are audited (`09-auth-teams-security.md`).

## Evidence

- Decisions "Audit log is insert-only by design" and "Audit log visibility is hardcoded Owner/Admin-only, not part of toggleable matrix".
