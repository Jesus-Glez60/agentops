# 0044. Reuse module labels while the scanned file set is unchanged

Amends the module-labeling call behind ADR 0005's doc page. Follows `findings.md` F24; the author decided on 2026-10-13.

## Status

Accepted, 2026-10-13.

## Context

`persist_doc_page` asks the default-tier model to group files into modules on every scan when an Anthropic key is set. A council audit chose the default tier on purpose (2026-09-28), but `agentops watch`, webhook reindexes and repeat scans paid for the same call over an unchanged tree.

## Decision

Keep the default tier. Store the labels and a SHA-256 of the sorted ranked file paths in the persisted `DocPage` (`module_set_hash`, `module_labels`). The next scan reuses the labels while the hash matches, and calls the model only when the file set changes. Labels are cached only when a call produced some.

## Alternatives considered

- The cheap tier. Rejected by the author; label quality is visible to users.
- No LLM call during scans at all. Rejected by the author.

## Consequences

- A change inside existing files keeps the old labels until a file is added, removed or re-ranked.
- Pages saved before this change have no hash, so their next scan makes one call.

## Evidence

- `findings.md` F24.
- `persist_doc_page` and `module_set_hash` (`agentops-core/crates/agentops-mcp/src/docgen.rs`); `DocPage` (`agentops-core/crates/agentops-docgen/src/model.rs`).
