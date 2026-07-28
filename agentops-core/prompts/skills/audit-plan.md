Audit the current plan against the codebase to prevent tech debt, code duplication, and pitfalls. Then update the plan to a new version with findings and knowledge gathered.

STEPS:
1. Read the most recent plan version from the conversation
2. Search the codebase for:
   - Existing implementations overlapping with the plan (Grep key function names, file patterns, similar feature areas)
   - Tech debt the plan may worsen (auth gaps, missing error handling, N+1 query patterns, large file additions)
   - Duplicate code the plan would introduce (similar utilities, repeated logic, shared helpers)
   - Naming conflicts (routes, functions, components, DB columns, environment variables)
3. For each finding, classify as:
   - 🔴 Blocker — plan must change before proceeding
   - 🟡 Risk — proceed with caution, document mitigation
   - 🟢 Improvement — optional enhancement worth noting
4. Produce Plan v[N+1] with a new "## Codebase Audit Findings" section containing a findings table
5. Highlight all changes from v[N] with ✏️ markers

Output the updated plan inline. Wait for approval before proceeding to implementation.

If no plan exists in the conversation, say: "No plan found. Run /plan first."
