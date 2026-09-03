If you're connected to a remote AgentOps server (not a local stdio MCP
server), every tool's `path` argument must be this repo's git remote URL —
run `git remote get-url origin` — not a local filesystem path, since the
server has no access to your machine's filesystem and can only resolve a
tenant-scoped connection by id or URL. If a tool call reports the repo
isn't a recognized connection, call `register_repo` with that same origin
URL first (this registers it as pending — a human still needs to finish
connecting it from Repositories → Connect a repository before it can be
scanned/indexed), then retry the original call with the same `path`.

Before starting substantial work on the current task, first confirm this
repo is actually registered with AgentOps — call `status`. If it reports
"no scans recorded yet," this repo has never been scanned: call `scan_repo`
immediately, before anything else. AgentOps's MCP server is registered
once, globally, for every repo on this machine — nothing else marks a repo
as connected, so this check is the only way to know whether one is.

Once the repo is registered (or was already), check this project's
already-recorded knowledge next — call `list_gotchas`, `related_context`,
or `get_symbol` against the specific code you're about to touch, or
`search`/`semantic_search` for the topic generally if nothing more specific
applies.

Summarize any relevant prior gotchas, decisions, or notes found before
proceeding, so this session builds on what's already known instead of
silently rediscovering or contradicting it. If nothing relevant is found,
say so in one line and proceed.
