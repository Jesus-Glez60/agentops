Design an implementation plan for the current task.

Before proposing any new code, apply this project's reuse-before-writing
decision ladder (`AGENTS.md`'s `## Code style` section) to each piece of
the design: does it need to exist at all, is it already in this codebase
(check via `Grep`, `get_symbol`, or `related_context` — don't assume), does
the standard library cover it, does a native platform feature cover it,
does an already-installed dependency cover it. If a dependency is the reuse
target but its docs aren't available (`get_docs`/`search_docs` come up
empty), see the `library-docs` skill for the lookup order — this is the
moment that gap is most likely to surface.

If any part of this task is delegated to a spawned subagent (an Explore
pass, a design sub-task), its prompt must tell it to check this project's
own recorded knowledge first — `get_session_guide`/`related_context`/
`list_gotchas` scoped to whatever files, symbols, or topic it's assigned —
before it starts grepping or reading raw source. A subagent that starts
cold re-derives, at real token cost, exactly what this project's own tools
already have indexed; the same reuse-first instinct this skill requires of
you applies to whatever you delegate.

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
