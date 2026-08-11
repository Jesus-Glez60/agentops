---
title: "nested Tokio runtime panic under PostgresGraphStore"
type: gotcha
---

agentops-cli's #[tokio::main] wrapped every subcommand in an ambient Tokio runtime. PostgresGraphStore owns its own internal tokio::Runtime and calls block_on per query (needed because GraphStore's trait is sync but tokio-postgres isn't) — this panicked outright with 'Cannot start a runtime from within a runtime' on every single Postgres-backed command (install, status, changelog, search, note, docgen). Only surfaced via live testing against a real Postgres container, invisible to any unit test. Fixed by making main() synchronous; only serve-api/docbrain-serve-api build their own runtime locally where genuinely needed.
