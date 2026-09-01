Run this project's reuse-before-writing decision ladder (see `AGENTS.md`'s
`## Code style` section) against the current uncommitted changes.

If a `ponytail-auditor` subagent is available, delegate to it directly.
Otherwise perform the same check yourself: for each new function, module,
or dependency in `git diff`, verify in order — does this need to exist at
all, is it already in this codebase, does the standard library cover it,
does a native platform feature cover it, does an already-installed
dependency cover it, would a one-line change have been enough. Verify each
claim with a real `Grep`/`Read`, not a guess.

Report only violations found, each with the specific existing alternative
you verified — or a single line confirming none were found. Do not modify
code and do not evaluate anything outside the ladder.
