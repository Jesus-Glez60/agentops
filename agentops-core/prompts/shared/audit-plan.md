Audit the current plan against this project's reuse-before-writing decision
ladder and recorded knowledge — before implementation starts, not after.

For each planned new component: was "already in this codebase" actually
checked via `Grep`/`get_symbol`/`related_context`, or just assumed? Does
any recorded gotcha or decision (`list_gotchas`/`related_context`) conflict
with, or already cover, part of this plan?

Report only violations or conflicts found — or confirm in one line that the
plan is clear to implement. Do not evaluate anything outside the ladder and
recorded knowledge (no general design review).
