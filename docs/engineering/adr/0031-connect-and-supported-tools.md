# 0031. connect: MCP registration plus Ruler distribution for five tools

Survey candidate T1. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

Connecting a coding tool was a buried `agentops install --agents` flag.

## Decision

`agentops connect` registers `agentops-mcp-server`, not the narrower `agentops serve`. It distributes instructions through a shared `distribute_via_ruler`, with a picker for Claude Code, Cursor, Codex CLI, Gemini CLI and Windsurf.

## Alternatives considered

- Cline, Zed and VS Code Copilot as first-class adapters. Rejected after checking each tool's docs. Cline's path is inside VS Code extension storage; Zed uses a different config key and shape; Copilot has no single user-level path.

## Consequences

- Other Ruler-supported ids still get instructions, but the user registers MCP by hand.
- Ruler needs `npx`; `connect` checks for it first.

## Evidence

- Decisions "agentops connect shipped: MCP registration + instructions distribution for Claude Code/Cursor/Codex/Gemini CLI" and "Cline/Zed/VS Code Copilot deliberately excluded from connect's first-class adapter list -- Windsurf added instead".
