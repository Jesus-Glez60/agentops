Start a new engineering session.

STEPS:

1. Read `AGENTS.md` from the project root. Extract: `ORG`, `PROJECT`, `STACK`, `VAULT_PATH`.
   If missing: "No AGENTS.md found. Run `nu scripts/link-project.nu` or create one manually." Stop.

2. Read recent vault notes:
   - Last 3 progress entries from `VAULT_PATH/progress/`
   - Any gotchas from `VAULT_PATH/gotchas/`

3. Query codebrain for project context (use MCP tool if available, otherwise curl):

   MCP: `get_project_context(org="[ORG]", project="[PROJECT]")`

   REST fallback:
   ```bash
   curl -s -X POST http://192.168.1.74:8001/project/context \
     -H "Content-Type: application/json" \
     -d '{"org": "[ORG]", "project": "[PROJECT]"}'
   ```

   If codebrain is unreachable, skip silently and continue.

4. Report session header:

```markdown
## Session Start — [PROJECT] ([ORG])

**Stack:** [STACK]
**Vault:** [VAULT_PATH]

### Vault Context (codebrain)

#### Decisions
- [[slug]] — one-line summary

#### Gotchas
- [[slug]] — one-line warning

#### Knowledge
- [[slug]] — one-line description

### Recent Progress
[last 3 progress entries, one line each]
```

5. Ask: "What are we working on today?"
