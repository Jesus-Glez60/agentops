#!/usr/bin/env bash
# PreToolUse hook (Write|Edit): blocks writing a note file under
# .agentops/notes/ whenever this repo is connected to a real remote
# AgentOps server (.context/agentops-remote.json exists) — in that case
# add_note/ingest_notes over MCP is always reachable and a local file
# write only produces an orphaned note. See docs/vault-protocol.md.
#
# This exists because the prose version of this rule (in CLAUDE.md,
# vault-archivist.md, save-note/wrap's SKILL.md) has been silently
# ignored by agents/subagents repeatedly -- this is the hard technical
# gate that doesn't depend on any of them reading it correctly.

set -euo pipefail

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

# Normalize Windows backslashes before matching, per Claude Code's own
# hooks docs (a forward-slash check never matches a backslash path).
NORMALIZED_PATH="${FILE_PATH//\\//}"

REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
MARKER="$REPO_ROOT/.context/agentops-remote.json"

if [[ "$NORMALIZED_PATH" == *"/.agentops/notes/"* ]] && [[ -f "$MARKER" ]]; then
  cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"This repo is connected to a real remote AgentOps server (.context/agentops-remote.json exists). Never write a note file directly under .agentops/notes/ -- call the add_note or ingest_notes MCP tool instead (see docs/vault-protocol.md). A local file here would just be an orphaned note nobody's dashboard will ever see."}}
JSON
fi

exit 0
