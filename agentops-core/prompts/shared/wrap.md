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

Then run the notes sweep: capture anything not already written
incrementally as its own note. Check `.context/agentops-remote.json`
first — if it exists, use `add_note`/`ingest_notes` (directly, or by
delegating to the `vault-archivist` subagent if available; never a direct
file write in this case). Only write a local Markdown file under
`NOTES_PATH` if that marker is absent.

Then run the reuse-before-writing audit against all of this session's
uncommitted changes — delegate to the `ponytail-auditor` subagent if
available, otherwise perform the same check directly (see
`ponytail-audit`'s own instructions).

Then run the same review-depth gate `audit-plan` uses, against this
session's `git diff` instead of plan text: compute the size/risk gate
(fewer than 3 files touched, under ~80 changed lines, nothing on an
auth/payment/data path) and spawn either one `plan-council-member` in
**solo playbook mode** (the common case — its own report, including Proof
for each surviving finding, is the verdict; no chair needed) or, at/above
the threshold, all five `plan-council-member` subagents in parallel in
**council mode** (Skeptic, Simplifier, Completeness-checker, Outsider,
Opportunity-hunter, each blind to the other four) plus one
`plan-council-chair` synthesizing their critiques, with an explicit ask for
structured, evidence-grounded disagreement between them rather than simple
merging. (A live test of an earlier 3-persona-only version of this gate
found that dropping Completeness-checker/Simplifier and the falsification
pass from the large/risky path removed exactly the scrutiny that should
scale up with risk, not down — the full 5-member council's cross-member
disagreement synthesis is what substitutes for solo mode's self-
falsification step here, so don't shrink this side again without
preserving that property.) Fall back to performing the relevant pass(es)
yourself in sequence if these subagents aren't available.

Report: notes saved (with exact paths), the ladder-audit result (violations
found, or a one-line confirmation of none), and the review verdict (the
solo playbook's report, or the council's consensus verdict — clear, or the
consolidated required changes).
