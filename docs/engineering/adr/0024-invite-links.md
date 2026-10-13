# 0024. Invites are copyable links

Survey candidate S5. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-17 (`1ec773a`).

## Context

There is no email infrastructure in the codebase.

## Decision

`POST /team/invites` returns a token; the UI builds a copyable `/invite/{token}` link. The admin sends it through any channel.

## Alternatives considered

- Email delivery. Deferred: SMTP or a provider, templates, bounce handling and compliance are premature for a self-hosted tool. Faking a sent email was explicitly rejected.

## Consequences

- The invite page is public; the proxy matcher must exclude it (gotchas 54056, 54031).

## Evidence

- Decision "Team invites are copyable links, no email infrastructure".
