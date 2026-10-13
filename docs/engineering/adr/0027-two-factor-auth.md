# 0027. TOTP 2FA with a separate challenge token

Survey candidate S9. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

Accounts needed second-factor login, and the TOTP secret must be stored recoverably.

## Decision

- TOTP secrets are encrypted with the existing per-tenant `integration_key` scheme, not a second mechanism.
- A correct password returns HTTP 202 with a challenge token valid for 5 minutes, not a session. `POST /auth/login/2fa` exchanges it plus a code for a real session.

## Alternatives considered

- A second encryption scheme. Rejected: two key derivations double the audit surface.
- A scoped-down session between password and code. Rejected: a leaked one would still grant some capability.

## Consequences

- A leaked challenge token grants nothing but code attempts for one user.
- A failed code leaves the challenge valid for retry, and there is no rate limit (`findings.md` F39).

## Evidence

- Decisions "2FA TOTP secret encrypted with same scheme as integrations" and "2FA login flow uses separate challenge token, not real session".
