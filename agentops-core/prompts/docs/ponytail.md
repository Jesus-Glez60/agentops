# PONYTAIL — Code Minimalism Decision Hierarchy

Before writing any code, walk this ladder. **Stop at the first rung that holds.**

1. **Does this need to exist?** (YAGNI) → If no: skip it, explain why
2. **Does the standard library solve it?** → Use it
3. **Does a native platform feature cover it?** → Use it
4. **Does an already-installed dependency solve it?** → Use it
5. **Can this be one line?** → Make it one line
6. **Only then:** write the minimum code that works

## Shortcuts Are OK — Mark Them

When choosing a shortcut over the ideal solution, tag it so it doesn't rot silently:

```
ponytail: <what's missing>, <upgrade path when it matters>
```

Examples:
```typescript
// ponytail: global lock, per-account locks if throughput matters
// ponytail: naive linear scan, add index when table exceeds 10k rows
```

## Non-Negotiables (never minimize these)

- Input validation at system boundaries (user input, external APIs)
- Error handling that prevents data loss
- Security (OWASP Top 10, auth, injection)
- Accessibility
- Explicitly requested features

## Active Mode: `full` (default)

Enforce all rungs. When proposing a solution, state which rung it hits and why you stopped there.

Switch modes with `/ponytail [lite|full|ultra|off]`:
- **lite** — build what's asked, name the lazier alternative in one line, user picks
- **full** — ladder enforced, all rungs checked (default)
- **ultra** — YAGNI extremist, delete before add, challenge the requirement itself
- **off** — normal mode, no minimalism enforcement
