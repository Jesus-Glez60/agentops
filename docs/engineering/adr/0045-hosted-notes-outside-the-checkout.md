# 0045. Hosted note files live beside the checkout, not in it

Amends ADR 0032. Follows `findings.md` F30; the author decided on 2026-10-13.

## Status

Accepted, 2026-10-13.

## Context

Hosted `add_note` wrote its Markdown file into the connection's checkout. Every indexing job deletes and re-clones the checkout, so the file was gone after the next reindex, and the scan-time re-match never saw it again.

## Decision

Each hosted connection's note files live in `<repo-checkouts>/.notes/<connection_id>--<tenant>` (`indexing::notes_dir_for_checkout`). Hosted `/mcp` sets `notes_path` to that folder for `add_note`, `ingest_notes` and `init_agents_md`; a caller still cannot pass `notes_path` (ADR 0036). Each indexing job points the fresh clone's `.agentops/config.json` at the folder, so `persist` re-matches the notes on every scan. Deleting a repository or an org removes the folder.

## Alternatives considered

- Re-matching from the graph's note nodes instead of files. Not chosen; bigger change.

## Consequences

- Notes survive reindexes and attach to symbols even when written before the first index (gotcha 54035).
- Note files written before this change were already lost with their checkouts; their graph nodes remain.

## Evidence

- `findings.md` F30.
- `notes_dir_for_checkout`, `point_checkout_at_notes`, `remove_connection_notes` (`agentops-core/crates/agentops-heavy-api/src/indexing.rs`); `NOTES_TOOLS` (`agentops-core/crates/agentops-heavy-api/src/mcp_http.rs`).
