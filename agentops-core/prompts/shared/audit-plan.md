Audit the current plan against this project's reuse-before-writing decision
ladder and recorded knowledge — before implementation starts, not after.

For each planned new component: was "already in this codebase" actually
checked via `Grep`/`get_symbol`/`related_context`, or just assumed? Does
any recorded gotcha or decision specific to this plan's actual files,
symbols, or libraries (checked via targeted `list_gotchas`/`related_context`
calls, not a generic glance) conflict with, or already cover, part of this
plan?

Report the ladder/gotcha result from this first pass, then run the council:
spawn five `plan-council-member` subagents in parallel (or, if that
subagent isn't available, perform each pass yourself in sequence) against
the plan text, one per persona — Skeptic, Simplifier, Completeness-checker,
Outsider, Opportunity-hunter — each told only its own persona and not to
expect or wait on the other four. Once all five return, spawn one
`plan-council-chair` (or synthesize directly if unavailable) with the plan
text and all five critiques attached, and report its consensus verdict. If
the chair's required changes affect the plan file itself, apply them before
treating the plan as ready.

Report: the first-pass ladder/gotcha result, then the council's consensus
verdict (clear, or the consolidated required changes). Do not evaluate
anything outside the ladder, recorded knowledge, and what the council
itself surfaced (no unprompted general design review).
