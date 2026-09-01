Design an implementation plan for the current task.

Before proposing any new code, apply this project's reuse-before-writing
decision ladder (`AGENTS.md`'s `## Code style` section) to each piece of
the design: does it need to exist at all, is it already in this codebase
(check via `Grep`, `get_symbol`, or `related_context` — don't assume), does
the standard library cover it, does a native platform feature cover it,
does an already-installed dependency cover it.

Only propose genuinely new code for whatever's left after that check.
Present the plan naming the specific existing code and dependencies it
reuses, not just a list of new files to create.
