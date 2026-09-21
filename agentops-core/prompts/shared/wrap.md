End of session.

Before writing, reconstruct the full list of this session's distinct
findings by systematically reviewing — not recalling from memory — the
session's actual changes: every file touched (`git status`/`git diff`),
every error or unexpected result hit and how it was resolved, every design
decision made and why, every library/API constraint discovered that wasn't
already known. Check each one individually: if it changed a decision,
contradicted an assumption, or would have caused a real bug had it not been
caught, it's worth a note — don't decide "nothing to add" without walking
through this list first. A wrap that produces zero or one note after a
multi-step session is a sign this review was skipped, not a sign nothing
happened.

Then run the notes sweep from `docs/vault-protocol.md`'s "Session end"
step: capture anything not already written incrementally as its own note,
via `add_note` or by delegating to the `vault-archivist` subagent if
available.

Then run the reuse-before-writing audit against all of this session's
uncommitted changes — delegate to the `ponytail-auditor` subagent if
available, otherwise perform the same check directly (see
`ponytail-audit`'s own instructions).

Report: notes saved (with exact paths), and the audit result (violations
found, or a one-line confirmation of none).
