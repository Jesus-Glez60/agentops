Establish a project's quality bar as a written contract, before an agent
quietly lowers it. Use when no quality bar is written down, the user wants
to set up constraints or standards, an agent keeps silencing checks or
skipping tests to reach green, or an autonomous build loop is about to run
against a test suite the same agent wrote. Skip it for one-off scripts,
throwaway prototypes, or when the project already has a `CONSTRAINTS.md`
the user isn't changing — read and follow that instead.

Detect before asking: check package/dependency manifests, the existing test
runner and linters, current coverage, CI config, and the agent harness
already in place. Report what you found, then ask only what's left — at
most four questions, one at a time (per `interview-me`'s discipline), each
with a default so "I don't know" still produces a working config: which
dimensions to enforce beyond the floor (coverage, security, performance,
accessibility, architecture), whether a failing check should block or warn,
whether to measure-and-hold today's numbers or target a specific one, and
the slowest check tolerable before handing work back.

Write `CONSTRAINTS.md` at the repo root: a floor that's always enforced (no
new suppression comments, no unimplemented stubs, no skipped or deleted
tests without a reason, no secrets), then a table of enforced dimensions
each naming the exact command that checks it — a dimension with a number
and no command is an aspiration, not a constraint. Add a measured-but-not-
yet-enforced section for anything without a number yet: record today's
value and refuse to get worse (a ratchet, not an invented target). Point
`AGENTS.md`/`CLAUDE.md` at the file with one line: read it before writing
code, don't weaken it to make a change pass.

Wire checks to where they're cheap: fast, diff-scoped checks in the edit
loop, coverage and related tests at task end, everything else in CI. At
review time, watch the diff for the five ways a bar gets quietly weakened:
a threshold moved, a test got easier, a checker got silenced (a new
`@ts-ignore`/`eslint-disable`/`nosemgrep`), a stub or empty catch stood in
for real work, or an exception appeared with no discussion. Tightening the
bar should be silent; loosening it should be loud.
