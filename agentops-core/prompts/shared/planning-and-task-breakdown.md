Break a spec or a large task into ordered, verifiable units before writing
code, when a task feels too large to start, needs estimating, or can be
parallelized. Skip it for single-file changes with obvious scope.

Read the spec and codebase read-only first — no code — and map the
dependency graph (what has to exist before what). Slice vertically, not
horizontally: prefer "user can create an account" (schema + API + UI
together) over "build all the schema, then all the API, then all the UI" —
each slice should leave the system in a working, testable state.

Size tasks so one fits a single focused session (roughly 1-5 files); if you
can't state its acceptance criteria in three bullets, or it touches two
unrelated subsystems, break it down further. Each task needs: a
one-paragraph description, specific testable acceptance criteria, a
verification step (the actual test/build command, not "tests pass"), its
dependencies, and the files it will likely touch.

Record tasks via this project's own `create_task`/`update_task_status`/
`list_tasks` MCP tools, not a separate markdown or external-tracker
convention — AgentOps already has a native task manager; don't duplicate
it. Keep any free-form design notes (architecture decisions, risks, open
questions) in whatever plan-mode file convention the current harness
already provides, or as a `decision`/`knowledge` note via `save-note`
otherwise.

Add checkpoints after every 2-3 tasks (tests pass, build clean, the
relevant end-to-end flow works) before continuing. Never bulk-close or
silently replan another session's unchecked tasks — if open tasks exist for
different work, stop and ask rather than assuming they're stale.
