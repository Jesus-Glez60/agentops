Save a project note for the current finding.

Read `AGENTS.md` and extract `NOTES_PATH`. If it's missing, stop and ask —
do not guess a path.

If a `vault-archivist` subagent is available, delegate the save to it with
the title/body/type inferred from the request above. Otherwise call the
`add_note` MCP tool directly, or — only if no MCP connection is reachable —
write a frontmattered Markdown file under `NOTES_PATH` following
`docs/vault-protocol.md`'s template.

Confirm with the exact path written and how (`add_note` vs. direct file
write).
