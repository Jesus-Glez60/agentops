# Spike: Claude Code hook behaviour for milestone 1

Item 0 of the milestone 1 spec (task-start flow). It was run on 2026-10-14 against Claude Code **2.1.286**, in a live session in this repo. No code from the spike is merged. The probes were throwaway command hooks added to `.claude/settings.local.json`, which was restored from a backup afterwards. Each one logged its stdin payload.

Hook config changes took effect mid-session, without a restart.

## Answers

| # | Question | Answer | Evidence |
|---|---|---|---|
| 1 | Does `UserPromptSubmit` carry `permission_mode`, and does it read `"plan"` in plan mode? | **Yes.** It reads `"plan"` in plan mode and `"auto"` otherwise. `Stop`, `PreToolUse` and `PermissionRequest` carry it too. | A logged payload: `"permission_mode":"plan","hook_event_name":"UserPromptSubmit","prompt":"spike: plan adding a comment to README"` |
| 2 | Does `SubagentStart` fire for Explore, what does it match on, and does its `additionalContext` reach the subagent? | **Yes, on all three.** The payload has `agent_id` and `agent_type` (`"Explore"`), and matchers filter on `agent_type`. A marker string returned as `additionalContext` was quoted back by the subagent. | The payload `"agent_type":"Explore"`, plus the subagent's report: "I found one marker. It came from SubagentStart hook context…" |
| 3 | Which hook sees the plan when it is presented? | **`PreToolUse` and `PermissionRequest`, both matched on `ExitPlanMode`.** Both receive `tool_input.plan` (the full plan text) and `tool_input.planFilePath`. **A `PreToolUse` `permissionDecision: "deny"` on `ExitPlanMode` is obeyed.** The plan is not shown to the user, the agent receives the deny reason, and it can act on it and call `ExitPlanMode` again. A plan the user rejects fires no `Stop` and no `PostToolUse`. | The probe denied the first call and allowed the second. Its log shows `already_denied=no`, then `already_denied=yes`, and only the second call produced a `PermissionRequest` and reached the user. |
| 4 | Does `Stop` input carry `stop_hook_active`? | **Yes**, alongside `last_assistant_message`, `permission_mode` and `background_tasks`. | Every logged `Stop` payload |

## Consequences for the plan

- **The requirement's "no hard gate" premise doesn't hold on 2.1.286.** It cited issue reports that `PreToolUse` doesn't fire on `ExitPlanMode` and that a deny is ignored. On this version it fires and the deny works. Item 6 should use a `PreToolUse` hook on `ExitPlanMode` that denies once with a reason and allows the next call. That is the spec's "nudge once, then warn" behavior with no `Stop` hook and no transcript parsing. It stays soft enforcement in effect, because the second call always goes through. Since this rests on one version, the hook must also fail open if `tool_input.plan` is missing.
- **Background-task completions arrive as `UserPromptSubmit`**, with a `prompt` starting `<task-notification>`. They are not user prompts. Item 2 must skip them, both when recording `user_intent` and when deciding whether a planning episode or recall seed starts.
- **Subagents receive no CLAUDE.md or AGENTS.md.** Both Explore probes reported none loaded. Their startup context held only the account and attribution reminders. For subagents, a `SubagentStart` hook is the only channel that delivers project memory.
- `SubagentStart` context is placed after the subagent's task prompt, not before it as the docs say. It is still read.

## Extra questions (added for the deviations approved on 2026-10-14)

| # | Question | Answer | Evidence |
|---|---|---|---|
| 5 | Does a `PreToolUse` deny stop a `Write` into `~/.claude/projects/*/memory/`? | **Yes.** The write was refused with the hook's reason, and the file was never created. | A probe `Write` to `memory/m1-spike-probe.md` returned the deny reason, and `ls` showed no such file. |
| 6 | Can Claude Code's auto-memory (MEMORY.md) be turned off? | **Likely yes:** the `autoMemoryEnabled: false` setting or the `CLAUDE_CODE_DISABLE_AUTO_MEMORY` env var. **Unverified:** both were found in the 2.1.286 binary's code (`disabled_by_env_var` / `disabled_by_setting: autoMemoryEnabled===false`), not in the docs, and neither was exercised. An `autoMemoryDirectory` setting also exists. | `strings` on `/opt/homebrew/Caskroom/claude-code/2.1.286/claude` |

## Found along the way

- **The agentops `hook-capture` hooks are not installed in this repo.** Neither `.claude/settings.json`, `.claude/settings.local.json` nor `~/.claude/settings.json` has them. The only project hook is the hand-written `.claude/hooks/block-local-notes.sh`. So pinned-note re-injection, session capture and the session guide have never run in this repo's sessions, whatever their routing.
