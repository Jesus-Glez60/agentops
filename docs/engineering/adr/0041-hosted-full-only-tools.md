# 0041. Full-only network and destructive tools on hosted `/mcp`; `libraries.manage` for the web

Follows `questions.md` Q6. Rationale comes from the author's answer A-8 and the decision of 2026-10-14 on the local server.

## Status

Accepted, 2026-10-14.

## Context

Only `agentops-mcp`'s own tool table knew about the Advisor/Full access mode. On hosted `/mcp`, `unregister_repo` (destructive) and docbrain's `scrape_library` and `sync_changelogs` (network fetches and cost on the shared server) ran in Advisor mode, the default. The web Libraries page calls `scrape_library` over REST, gated only by `libraries.view`, which viewers have.

## Decision

1. Hosted `/mcp` hides and refuses `unregister_repo`, `scrape_library` and `sync_changelogs` in Advisor mode (`HOSTED_FULL_ONLY_TOOLS` in `mcp_http.rs`). The refusal names the fix: an org admin switches the org to Full.
2. The local stdio server does not gate them: its user owns the machine and pays for its own fetches.
3. The web route `POST /libraries/tools/{name}` requires a new capability, `libraries.manage` (admin and member by default), for `scrape_library` and `sync_changelogs`. The org's access mode does not apply to the web.
4. `persist_blueprint_doc`, `register_library` and `discover_library` stay available in Advisor mode, as knowledge writes.

## Alternatives considered

- Gating the web route by the org's access mode. Rejected: orgs default to Advisor, so web scraping would disappear for every default org.
- Gating the local stdio server too. Rejected by the author: every solo user would lose scraping unless they set `AGENTOPS_ACCESS_MODE=full`.
- Moving an access concept into `docbrain-mcp`. Not done: one gated surface does not justify it, and docbrain stays free of access concepts (`agentops-mcp-server` module doc).

## Consequences

- An MCP client in an Advisor org can no longer scrape docs; a member can still do it from the web.
- Custom roles created before this decision lack `libraries.manage` until an admin adds it.

## Evidence

- Author answer A-8, `questions.md`.
- `HOSTED_FULL_ONLY_TOOLS` (`agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`); `MANAGE_TOOLS` (`agentops-core/crates/agentops-heavy-api/src/libraries_http.rs`); `CAP_LIBRARIES_MANAGE` (`agentops-core/crates/agentops-teams/src/lib.rs`).
