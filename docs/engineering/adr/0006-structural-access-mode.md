# 0006. Structural Advisor/Full access mode, Advisor by default

Survey candidate M1. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-07-28 (`33b4aaa`).

## Context

MCP clients can be any editor or agent, from vim and nvim to VS Code and Cursor. The server cannot know or trust which one is calling.

## Decision

Every core tool declares `access: Advisor | Full` in one `ToolSpec` row. Listing, dispatch and gating all derive from that row. `list_tools(mode)` hides Full-only tools in Advisor mode, and `call_tool` also refuses them. The default mode is Advisor.

## Alternatives considered

- Three hand-synced lists for listing, dispatch and access. Replaced, because they could drift (`tool_specs` doc comment).
- Hide only, or refuse only. The code does both; refusing is a "defensive backstop" for clients that call unlisted tools.

## Consequences

- A safe default for an untrusted client.
- Only the core table is gated; docbrain and semantic-index tools are not (`questions.md` Q6).
- The mode is not a permission system: it applies to every caller in an org (`09-auth-teams-security.md`).

## Evidence

- Author answer `questions.md` A-5 (2026-10-12).
- Comment on `AccessMode::Advisor` and the `tool_specs` doc comment in `agentops-core/crates/agentops-mcp/src/tools.rs`.
- Commit `33b4aaa`.
