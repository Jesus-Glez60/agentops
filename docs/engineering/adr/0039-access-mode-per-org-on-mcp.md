# 0039. Access mode per org on hosted `/mcp`, per process on REST `/tools`

Survey candidate M3. Rationale comes from the author's answer A-7 in `questions.md`.

## Status

Accepted. Per-org mode added in `ee9b7e9` (2026-08-31). Rationale recorded 2026-10-13.

## Context

Hosted `/mcp` reads `organizations.mcp_access_mode` on every call. REST `/tools` on the same process reads `AGENTOPS_ACCESS_MODE` once at start-up.

## Decision

Keep both. REST `/tools` accepts only the instance key, so its caller is the operator and one process-wide mode fits. Hosted `/mcp` serves many orgs, so each org's admin sets its mode.

## Alternatives considered

- One mode per API key. Not chosen; the org is the unit that decides what its agents may do.

## Consequences

- An operator sets `AGENTOPS_ACCESS_MODE` for REST; org admins set their own `/mcp` mode on the Team page.
- Hosted `/mcp` also gates three tools outside the core table by this mode (ADR 0041).

## Evidence

- Author answer A-7, `questions.md`.
- `resolve_access_mode` (`agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`); `require_api_key` on `/tools` (`agentops-core/crates/agentops-api/src/lib.rs`).
