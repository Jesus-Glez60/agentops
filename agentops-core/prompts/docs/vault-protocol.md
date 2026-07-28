# VAULT PROTOCOL

## ABSOLUTE RULES — Never deviate

1. **ALWAYS read `AGENTS.md`** before any vault write. Extract the exact `VAULT_PATH`.
2. **NEVER write to a vault path** that differs from the one in `AGENTS.md`.
3. **NEVER create vault files without YAML frontmatter** (see template below).
4. **If `AGENTS.md` is missing or has no `VAULT_PATH`**: STOP. Ask the user for the path. Do not guess.
5. **ALWAYS delegate vault writes to vault-archivist**. Domain agents never write directly.
6. **NEVER invent slugs** — derive them from the task title in lowercase-hyphenated form.

---

## Path Mapping

Read `VAULT_PATH` from `AGENTS.md`. All paths below are relative to it.

| Content | Path | Naming |
|---------|------|--------|
| Session notes | `progress/YYYY-MM-DD-session.md` | Date prefix required |
| Bugs / workarounds | `gotchas/{slug}.md` | kebab-case slug |
| Reusable patterns | `knowledge/{slug}.md` | kebab-case slug |
| Architecture choices | `decisions/{slug}.md` | kebab-case slug |
| Codebase reviews | `reviews/YYYY-MM-DD-review.md` | Date prefix required |

---

## Required YAML Frontmatter

Every vault file must start with this block. Derive values from `AGENTS.md`.

```yaml
---
title: "[Descriptive title]"
project: "[PROJECT from AGENTS.md]"
organization: "[ORG from AGENTS.md]"
type: progress|gotcha|knowledge|decision|context
tags: []
status: evergreen
created: YYYY-MM-DD
---
```

Notes must end with a `## Related` section using wikilinks where applicable.

---

## After Writing

Always confirm:
> "Saved to vault:
> - `[exact path 1]`
> - `[exact path 2]`"

Report the line count of each file written.

---

## Vault Archivist Delegation Pattern

Domain agents use this at STEP 7:

```
Spawning vault-archivist...

Save the following:
- progress/YYYY-MM-DD-session.md — [summary of what was done]
- gotchas/[slug].md — [describe the bug/workaround if any]
- knowledge/[slug].md — [describe the pattern if any]
- decisions/[slug].md — [describe the choice if any]

VAULT_PATH: [value from AGENTS.md]
ORG: [value]
PROJECT: [value]
```
