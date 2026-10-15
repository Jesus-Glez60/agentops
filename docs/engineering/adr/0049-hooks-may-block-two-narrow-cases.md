# 0049. The PreToolUse hook blocks in two narrow cases

Milestone 1 of the cue-driven memory work (task-start flow), 2026-10-14. Approved with the milestone 1 plan. The spike results are in `docs/engineering/spikes/m1-hooks.md`.

## Status

Accepted, 2026-10-14.

## Context

`agentops hook-capture --event pretooluse` has never blocked a tool call. Its secret-exposure warning and `--compress` rewrite change what the agent sees or runs, never whether it runs. That kept the hook safe to install by default.

Milestone 1 needs two rules that prose alone hasn't held:

1. **A presented plan carries "Research performed" and "Council verdict" sections.** The requirement had planned only a Stop-hook check, because public issue reports said a deny on `ExitPlanMode` is ignored. The spike showed that on Claude Code 2.1.286 `PreToolUse` fires on `ExitPlanMode` with the full plan in `tool_input.plan`, and a deny is obeyed.
2. **Memory is written to AgentOps, not to Claude Code's local MEMORY.md** (item 7). The spike showed a `PreToolUse` deny on a `Write` into `~/.claude/projects/*/memory/` stops it.

## Decision

`pretooluse` may return `permissionDecision: "deny"` in exactly these two cases. Every other path keeps the never-blocks rule.

- `ExitPlanMode` with a required section missing is denied once per planning episode (`ENFORCE_MAX_NUDGES`). The reason names the missing step. The next attempt goes through with a `systemMessage` warning to the user. It fails open when `tool_input.plan` is absent. This is a nudge, not a gate, so a plan can never be stuck.
- `Write`/`Edit`/`MultiEdit` into Claude Code's memory directory is denied, with a reason that redirects the write to `add_note`.

## Alternatives considered

- A Stop hook that flags a plan missing its sections afterwards. Not chosen: a rejected plan fires no Stop (spike), and by then the user has already seen the plan.
- Prose only (skills and `AGENTS.md`). Not chosen: that's the "recalled but not followed" failure the requirement exists to fix, and subagents don't load `AGENTS.md` at all (spike).

## Consequences

- If a later Claude Code version stops passing `tool_input.plan`, enforcement silently turns off instead of blocking.
- The memory-directory deny holds only in repos where the hooks are installed.

## Evidence

- `docs/engineering/spikes/m1-hooks.md` (Q3, Q5).
- `check_presented_plan` and `pre_tool_use` in `agentops-core/crates/agentops-mcp/src/hooks.rs`.
