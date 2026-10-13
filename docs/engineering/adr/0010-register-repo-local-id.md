# 0010. Register repos that have no git remote

Survey candidate M6. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

An agent working in a repo AgentOps has never seen should be able to register it without a person opening the web UI. Some repos have no git remote at all.

## Decision

`register_repo` accepts either `repo_url` or `local_id`. A `local_id` registers a permanently local-only connection under a synthetic `local:` URL. Such a repo gets notes and scans from the developer's CLI, and no server-side clone is ever expected. Registered repos start as `Discovered`, `Pending`.

## Alternatives considered

- Require a git remote. Not possible for these repos: "there's nothing `normalize_repo_path` could ever accept" (doc comment).

## Consequences

- `local_id` is restricted to 1–128 letters, digits, `-` and `_`, because it becomes a path component.
- A local-only stub is never auto-attached to a GitHub connection on a name match; merging needs a person (hosted decision "Never auto-attach a Discovered local-only stub on a name match alone").
- Merging can leave a client's marker pointing at a dead id (gotcha 54243).

## Evidence

- Hosted decision "register_repo now accepts local_id for repos with no git remote".
- Doc comment on `register_repo` in `agentops-core/crates/agentops-heavy-api/src/tenant_repo.rs`.
