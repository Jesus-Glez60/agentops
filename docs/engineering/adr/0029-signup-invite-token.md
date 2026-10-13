# 0029. Signup validates an invite without consuming it

Survey candidate S11. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

`AGENTOPS_SIGNUP_MODE=first-user-only` blocks signup once an account exists, unless an `invite_token` is presented. The existing invite flow already joins the user after login.

## Decision

`signup()` checks the token with `TeamStore::preview_invite`, which has no side effects. The join still happens once, later, through `POST /invites/accept`.

## Alternatives considered

- Accept the invite inside `signup()`. Rejected: `accept_invite` is single-use, so the later real accept would fail for every invited signup.

## Consequences

- `preview_invite` and `accept_invite` are not interchangeable.

## Evidence

- Decision "Signup's invite_token gates open signup, but must NOT accept/consume the invite itself".
