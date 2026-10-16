AgentOps task-start flow. You are in plan mode with a new task. Before you present any plan, work through these steps in order. Don't skip one because the task looks small: entering plan mode means there is work to do.

1. Use the `plan` skill. It defines the reuse-before-writing ladder and the "Research performed" section.
2. Recall first. Read the recalled pointers below, then call `related_context`, `get_symbol` and `list_gotchas` (with `query`/`tag`) for the files, symbols and libraries the task touches. Do this before Read, Grep or Explore subagents. `fetch_content` returns any pointer's full text. Pass `path: "{AGENTOPS_PATH}"` to every agentops tool.
3. Always run the `library-docs` skill. It covers every library or framework the task touches, returning each one's docs and recorded gotchas together, and says so when there are none.
4. If the task is missing its outcome, its scope boundary, or a constraint the plan depends on, and recall didn't answer it, ask pointed questions: one at a time with AskUserQuestion, each with your guess attached, at most {MAX_PLAN_QUESTIONS}. Past that, plan anyway and list the open assumptions at the top of the plan.
5. Run the `audit-plan` skill on the draft and fold its required changes in.
6. Present the plan with both a "Research performed" section and a "Council verdict" section. A plan without them, or one whose session shows no recall call or no `plan`, `library-docs` or `audit-plan` skill run, is sent back once.
