Write a structured spec before writing any code, when starting a new
project or feature, requirements are ambiguous, the change spans multiple
files, or an architectural decision is coming — skip it for single-line
fixes or changes where requirements are already unambiguous and
self-contained.

If one request actually bundles several independently testable
capabilities (distinct consumers or data, acceptance criteria that cluster
into separately-shippable groups), don't write one oversized spec — propose
a capability map first: a small table of stable, kebab-case module ids,
their dependencies, and a build order, gated on human review before any
module spec is written. Then run Specify → Plan → Tasks → Implement per
module in dependency order, each with its own spec named by module id.

Specify: surface your assumptions explicitly before writing anything
("ASSUMPTIONS I'M MAKING: ... → correct me now or I'll proceed with
these"), then write a spec covering objective, commands, project structure,
code style (one real snippet beats three paragraphs describing it), testing
strategy, and boundaries (always do / ask first / never do). Reframe vague
asks ("make it faster") into specific, testable success criteria before
treating the spec as done. If the project already uses an external spec
format (OpenSpec or similar), keep that format — this skill owns the
clarification and approval gate, not the file format.

Plan: identify components, dependencies, implementation order, risks, and
what can run in parallel vs. must be sequential — see
`planning-and-task-breakdown` for the dependency-graph and vertical-slicing
mechanics behind this, once that skill is available in this project.

Tasks: break the plan into units small enough for one focused session, each
with acceptance criteria and a verification step — record them via this
project's own `create_task`/`list_tasks`/`update_task_status` MCP tools,
not a `tasks/todo.md` file. AgentOps already has a native task manager;
don't stand up a second, file-based one alongside it.

Implement: execute tasks one at a time, and keep the spec itself alive —
update it when a decision or scope changes, don't just document after the
fact. A two-line spec is fine for something small; the point is forcing
clarity before code, not producing a long document.
