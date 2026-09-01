End of session.

First, run the notes sweep from `docs/vault-protocol.md`'s "Session end"
step: capture anything not already written incrementally as its own note,
via `add_note` or by delegating to the `vault-archivist` subagent if
available.

Then run the reuse-before-writing audit against all of this session's
uncommitted changes — delegate to the `ponytail-auditor` subagent if
available, otherwise perform the same check directly (see
`ponytail-audit`'s own instructions).

Report: notes saved (with exact paths), and the audit result (violations
found, or a one-line confirmation of none).
