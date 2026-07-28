# agentops

Codebrain + Docbrain: a memory layer for AI coding agents. Turns a codebase into a
graph of code structure, dependencies, and documented gotchas/decisions
("neurons"), and turns third-party library docs into version-exact, queryable
context ("molecules") — both MCP- and REST-API-native, so any agent (Claude Code,
Cursor, Codex, etc.) can connect.

**Status**: Phase 1 skeleton — crates compile and the CLI runs, but the actual
scanning/graph/docgen logic is not yet implemented (each crate is a stub, see its
`placeholder()` function). Not ready for real use yet.

Full design, feature spec, and phased rollout:
`/Users/jesusglez/.claude/plans/i-m-thinking-that-now-modular-sparrow.md`

Security posture and threat model: [`SECURITY.md`](./SECURITY.md).

## Repo layout

```
agentops-core/      # MIT/Apache — light tier: scanner, graph, docgen, notes,
                     #   security, manifest, ruler-bridge, agents-md, mcp, api, cli
docbrain-core/       # library docs/changelog ingestion, versioned + access-controlled
agentops-heavy/      # commercial — Docker Compose bring-up for Neo4j/Postgres/Qdrant
apps/web/            # Next.js 16 dashboard (non-technical-user facing)
tests/fixtures/      # synthetic repos for scanner unit + e2e tests
```

## Building

Rust workspace:

```
cargo build --workspace
cargo test --workspace
cargo deny check          # supply-chain / license / advisory gate — see deny.toml
```

Frontend:

```
cd apps/web && npm install && npm run dev
```

## License

Core crates (`agentops-core/`) are MIT OR Apache-2.0 — see the open-core split
described in the plan. `agentops-heavy/` is commercially licensed, not open source.
