# 0033. Web app under /suite; marketing site without login

Survey candidate T4. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-09-27 (`1206993`).

## Context

A public marketing site needed the domain root.

## Decision

The web app moves under a base path, default `/suite`, set by `AGENTOPS_WEB_APP_BASE_PATH` for both the app and the backend. The Astro marketing site owns the root and has no login or registration; calls to action link to the self-host quick start.

## Alternatives considered

- Hardcode `/suite` on each side. Rejected in favour of one env var (`next.config.ts` comment).

## Consequences

- Raw `fetch` calls need `withBasePath` (gotcha 53913).
- The marketing site's only backend call is the contact form.

## Evidence

- Hosted decision "Marketing site has no login/register flow by design".
- `apps/marketing/README.md`, `apps/web/next.config.ts`.
- Commit `1206993`.
