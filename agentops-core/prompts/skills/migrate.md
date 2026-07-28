Invoke the knowledge-migration agent to migrate raw project notes into the structured Obsidian vault.

Provide the source directory path containing the notes to migrate (e.g., `~/Downloads/project-notes/`). The agent will:
1. Scan the source directory for .md and .txt files
2. Present an Intake summary with a file list
3. Draft a Migration Plan showing classification (context/decision/progress/knowledge/gotcha) and target path for each file
4. Wait for approval before writing anything
5. Execute the migration: add YAML frontmatter, standardize headers, label code blocks, add Related sections
6. Generate a README.md index at the vault project root
7. Report a summary of files processed, created, and skipped

Classification heuristics:
- context → setup, architecture, environment notes
- decision → trade-offs, ADR-style, "we decided"
- progress → status updates, dated entries
- knowledge → how-tos, tutorials, patterns
- gotcha → bugs, workarounds, "watch out"

Use this skill when onboarding a new project with existing notes, or after a sprint to batch-import scratch notes.
