# WORKFLOW CONTRACT

These steps are MANDATORY for every session. Never skip or reorder them.

---

## STEP 1 — SESSION START

Read `AGENTS.md` in the project root. Extract: `VAULT_PATH`, `ORG`, `PROJECT`, `STACK`.

```bash
cat AGENTS.md 2>/dev/null
cat "$(grep VAULT_PATH AGENTS.md | cut -d: -f2 | xargs)/contexts/"*.md 2>/dev/null | head -50
cat "$(grep VAULT_PATH AGENTS.md | cut -d: -f2 | xargs)/progress/"*.md 2>/dev/null | tail -30
cat "$(grep VAULT_PATH AGENTS.md | cut -d: -f2 | xargs)/gotchas/"*.md 2>/dev/null
```

If `AGENTS.md` is missing:
> "No AGENTS.md found. Please run `nu ~/Repos/personal/dev-agent-ops/scripts/link-project.nu` or create one manually with: ORG, PROJECT, STACK, VAULT_PATH."

Do not proceed without VAULT_PATH.

---

## STEP 2 — INTAKE

**Trigger:** User describes issue, feature request, bug, or shares a file.

1. Analyze what was provided
2. Ask clarifying questions (max 3–5)
3. Summarize your understanding

**Output:**
```markdown
## Intake: [Brief Title]

### Understanding
[1–2 sentence summary of what's needed]

### Details Captured
- [Requirement 1]
- [Constraint or context]

### Clarifying Questions (if any)
1. [Question]?

### Ready to Draft Plan?
[Yes, proceeding... / Waiting for answers]
```

---

## STEP 3 — DRAFT PLAN v1

After intake is clear, produce the first draft. Domain-specific plan sections come from your domain analysis file.

**Required sections for all agents:**
```markdown
## Plan v1: [Task Name]

### Goal
[One sentence]

### Approach
1. [Step with file path]
2. [Step with file path]

### Files to Create/Modify
| File | Action | Purpose |
|------|--------|---------|
| `src/...` | Create/Modify | ... |

### Technical Decisions
- [Decision]: [Rationale]

### Risks & Unknowns
- [Risk or unknown needing investigation]

### Acceptance Criteria
- [ ] [Measurable criterion 1]
- [ ] [Measurable criterion 2]

---
**Feedback?** Tell me what to change, or say "approved" to proceed to codebase audit.
```

---

## STEP 4 — ITERATE PLAN

**Trigger:** User provides feedback.

1. Acknowledge feedback
2. Revise specific sections
3. Increment version (v2, v3…)
4. Highlight what changed with ✏️

```markdown
## Plan v[N]: [Task Name]

### Changes from v[N-1]
- ✏️ [What changed and why]

[...updated plan sections...]

---
**Feedback?** Or "approved" to proceed to codebase audit.
```

Repeat until user says: `"approved"`, `"go"`, `"implement"`, `"looks good"`, `"ship it"`.

NEVER skip to STEP 4.5 without explicit approval.

---

## STEP 4.5 — CODEBASE AUDIT (mandatory before every implementation)

After plan approval, scan the codebase before writing any code.

1. Search for existing implementations that overlap with the plan:
   ```bash
   grep -r "[key function/component names from plan]" src/ --include="*.ts" --include="*.tsx" --include="*.py" -l
   ```
2. Check for tech debt the plan may worsen: N+1 query patterns, missing error handling, auth gaps, files growing too large
3. Identify duplicate code the plan would create: similar utilities, repeated patterns, near-identical functions
4. Check for naming conflicts: existing functions, routes, DB columns, component names

**Output:** Plan v[N+1] with a new **"Codebase Audit Findings"** section:

```markdown
## Plan v[N+1]: [Task Name]

### Codebase Audit Findings
| Finding | Severity | Action Taken |
|---------|----------|--------------|
| Similar utility already in `src/utils/format.ts` | 🟡 Risk | Reusing instead of creating new |
| `UserService` already handles auth | 🔴 Blocker | Revised approach to extend it |
| No naming conflicts found | 🟢 Clear | — |

### Changes from v[N]
- ✏️ [What changed based on audit]

[...updated plan sections...]

---
**Audit complete. Proceed with implementation?**
```

---

## STEP 5 — FETCH DOCUMENTATION

Before implementing, fetch current docs for all libraries involved:

> "Fetching [library] docs via Context7..."

CRITICAL for: UI libraries, ORMs, auth systems, AI/LLM APIs, platform SDKs, any external package. APIs change — always fetch before implementing.

---

## STEP 5.5 — PONYTAIL CHECK (mandatory before every implementation)

Walk the decision ladder for each code item in the approved plan. Stop at the first rung that holds.

1. Does it need to exist? (YAGNI)
2. Standard library already does it? Use it.
3. Native platform feature covers it? Use it.
4. Already-installed dependency solves it? Use it.
5. One line? Make it one line.
6. Only then: write minimum working code.

Mark deliberate shortcuts: `ponytail: <what's missing>, <upgrade path>`

Output:
```markdown
### Ponytail Check
| Item | Rung Hit | Notes |
|------|----------|-------|
| Auth middleware | 6 — minimum code | Uses existing session lib |
| Rate limiter | 4 — existing dep | ponytail: global lock, per-account if needed |
```

---

## STEP 6 — IMPLEMENT

**Only after explicit approval of the audited plan.**

- Implement exactly what was approved — do NOT add unrequested features
- Show diffs for modifications, full files for new files
- Run validation (lint, typecheck)

```markdown
## Implementation Complete

### Files Created
- `src/...` — [purpose]

### Files Modified
- `src/...` — [change summary]

### Validation
- ✅ Lint passed
- ✅ Types passed

### Next Steps
- [ ] [Follow-up if any]

---
**Review the changes?** Or "wrap up" to save session to vault.
```

---

## STEP 7 — SESSION END

**Trigger:** "wrap up", "done", "save"

Spawn vault-archivist to write session outputs. Never write vault files directly.

Say:
> "Spawning vault-archivist to save session..."

Then describe what to save:
- What was accomplished (progress note)
- Any bugs or workarounds found (gotchas)
- Any patterns worth saving (knowledge)
- Any architectural choices made (decisions)
