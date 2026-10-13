#!/usr/bin/env bash
# Lists MCP tool names per source table, counted from source (not a live server).
# Composition per surface is documented in docs/engineering/survey/mcp-inventory.md.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

list() { grep -oE '^\s*name: *"[a-z_]+"' "$1" | grep -oE '"[a-z_]+"' | tr -d '"'; }

for f in agentops-core/crates/agentops-mcp/src/tools.rs \
         docbrain-core/crates/docbrain-mcp/src/tools.rs \
         agentops-core/crates/agentops-heavy-mcp/src/tools.rs \
         agentops-core/crates/agentops-heavy-api/src/mcp_http.rs; do
  names=$(list "$f")
  echo "== $f ($(echo "$names" | grep -c .))"
  echo "$names" | sed 's/^/  /'
done
