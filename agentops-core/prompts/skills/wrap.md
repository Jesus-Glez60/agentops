End the current session and save notes to the vault.

Invoke the vault-archivist to write a session progress note. Before delegating, summarize:
- What was completed this session
- What is in progress (with current state)
- Blockers or open questions
- Next steps

The vault-archivist will write to: `{VAULT_PATH}/progress/YYYY-MM-DD-session.md`

If any gotchas, decisions, or reusable patterns emerged this session, the archivist will also prompt to save those to the appropriate vault subfolder.

If AGENTS.md is missing or has no VAULT_PATH, the archivist will stop and ask before writing anything.

Use this skill at the end of every session to maintain a searchable dev journal.
