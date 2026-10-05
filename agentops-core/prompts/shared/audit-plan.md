Audit the current plan against this project's reuse-before-writing decision
ladder and recorded knowledge — before implementation starts, not after.

For each planned new component: was "already in this codebase" actually
checked via `Grep`/`get_symbol`/`related_context`, or just assumed? Does
any recorded gotcha or decision specific to this plan's actual files,
symbols, or libraries (checked via targeted `list_gotchas`/`related_context`
calls, not a generic glance) conflict with, or already cover, part of this
plan?

Report the ladder/gotcha result from this first pass, then decide how deep
a review this plan needs. Check the plan's own "Research performed" section
first (see the `plan` skill) — a plan missing that section, or one whose
listed files/symbols have changed since it was written, forces the deeper
path below regardless of size, since the cache this gate otherwise leans on
isn't trustworthy.

Size/risk gate: if the plan touches fewer than 3 files, proposes under ~80
lines of new/changed code, and nothing on an auth/payment/data path, spawn
one `plan-council-member` in **solo playbook mode** (see that agent's own
instructions) against the plan text — its own report, including its Proof
for each surviving finding, is the verdict; no chair is needed. Otherwise,
spawn all five `plan-council-member` subagents in parallel, one each in
**council mode** for Skeptic, Simplifier, Completeness-checker, Outsider,
and Opportunity-hunter (each told only its own persona, not to expect or
wait on the other four). Keep the full five here, don't reduce it: a live
test of a 3-persona-only version of this gate (Skeptic+Outsider+
Opportunity-hunter, dropping Simplifier/Completeness-checker and the
falsification pass for "efficiency") found this backwards — completeness/
edge-case scrutiny should scale up with risk, not down, and the full
council's cross-member disagreement synthesis is what substitutes for
solo mode's self-falsification step on this path. Once all five return,
spawn one `plan-council-chair` with the plan text and all five critiques
attached, explicitly asking it to surface structured, evidence-grounded
disagreement between them rather than just merging independent opinions,
and report its consensus verdict. Fall back to performing the relevant
pass(es) yourself in sequence if these subagents aren't available. If the
verdict's required changes affect the plan file itself, apply them before
treating the plan as ready.

Report: the first-pass ladder/gotcha result, then the review verdict (the
solo playbook's report, or the council's consensus verdict — clear, or the
consolidated required changes). Do not evaluate anything outside the
ladder, recorded knowledge, and what the review itself surfaced (no
unprompted general design review).
