Design an implementation plan for the current task.

Before proposing any new code, apply this project's reuse-before-writing
decision ladder (`AGENTS.md`'s `## Code style` section) to each piece of
the design: does it need to exist at all, is it already in this codebase
(check via `Grep`, `get_symbol`, or `related_context` — don't assume), does
the standard library cover it, does a native platform feature cover it,
does an already-installed dependency cover it. If a dependency is the reuse
target but its docs aren't available (`get_docs`/`search_docs` come up
empty), discover/register/scrape it yourself before continuing — see the
`session` skill's doc-gap instruction, this is the moment that gap is most
likely to surface.

Before finalizing the plan, always check for gotchas or decisions relevant
to *this task's* topic, symbols, or libraries — via `related_context` or
targeted `list_gotchas`/search calls scoped to the specific files, symbols,
or libraries this plan touches, not a generic glance. Do this even when the
task looks simple or unrelated to past work — that's exactly when a
relevant gotcha is easiest to miss. Name what was found, or state in one
line that nothing relevant exists. A plan that skipped this check is
incomplete.

Only propose genuinely new code for whatever's left after that check.
Present the plan naming the specific existing code and dependencies it
reuses, not just a list of new files to create.
