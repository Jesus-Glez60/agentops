# 0034. Linear auto-kickoff as a per-account module

Survey candidate T5. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

Linear webhook auto-kickoff first read one default tenant from a boot-time JSON file. Credentials added through the vault land in each user's own tenant, so the webhook never found them.

## Decision

Move the opt-in into `agentops-integration-modules`: a `module_enrollments` table (tenant, module, opaque JSON config, enabled), separate from the credentials vault. Linear auto-kickoff is its first consumer. The webhook verifies HMAC-SHA256 with a 60-second replay window and dedupes delivery ids.

## Alternatives considered

- Patch the single mismatch with an env var. Rejected: env-var fallbacks for these keys had just been deprecated in favour of the vault.

## Consequences

- Enrollment and secrets stay separate, as identity and secrets already are.
- Auto-kickoff's `sync_docs` call writes a global docbrain file (`findings.md` F36).

## Evidence

- Decisions "Phase 6 core implemented and live-verified: Linear webhook auto-kickoff" and "Phase 6c: auto-kickoff moved from boot-time config to per-account opt-in via agentops-integration-modules".
