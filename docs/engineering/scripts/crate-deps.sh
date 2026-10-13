#!/usr/bin/env bash
# Prints every workspace member and its internal (agentops-*/docbrain-*) deps,
# measured from Cargo.toml. Usage:
#   docs/engineering/scripts/crate-deps.sh            # "crate: dep dep ..." per member
#   docs/engineering/scripts/crate-deps.sh --check F... # fail if a member is missing from all files F
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

members=$(sed -n '/^\[workspace\]/,/^\]/p' Cargo.toml | grep -oE '"[^"]+/crates/[^"]+"' | tr -d '"')

if [[ "${1:-}" == "--check" ]]; then
  shift; missing=0
  for m in $members; do
    name=$(basename "$m")
    # A crate counts as present if it is backticked or is a "## <name>" heading.
    grep -qE "(\`$name\`|^## $name\$)" "$@" || { echo "missing: $name"; missing=1; }
  done
  [[ $missing == 0 ]] && echo "ok: all $(echo "$members" | wc -l | tr -d ' ') members present in: $*"
  exit $missing
fi

for m in $members; do
  name=$(basename "$m")
  # Only [dependencies]-style sections; dev-dependencies are listed separately.
  deps=$(awk '/^\[/{sec=$0} sec ~ /^\[(target.*\.)?dependencies\]/' "$m/Cargo.toml" \
    | { grep -oE '^(agentops|docbrain)-[a-z0-9-]+' || true; } | { grep -vx "$name" || true; } | sort -u | tr '\n' ' ')
  dev=$(awk '/^\[/{sec=$0} sec ~ /^\[(target.*\.)?dev-dependencies\]/' "$m/Cargo.toml" \
    | { grep -oE '^(agentops|docbrain)-[a-z0-9-]+' || true; } | { grep -vx "$name" || true; } | sort -u | tr '\n' ' ')
  echo "$name: ${deps% }${dev:+ | dev: ${dev% }}"
done
