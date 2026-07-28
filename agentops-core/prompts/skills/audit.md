Invoke the security agent to perform a security audit of the current codebase or a specific area.

Optionally specify a focus area (e.g., "auth flow", "API endpoints", "file uploads"). Without a focus, the audit covers the full OWASP Top 10 across the codebase.

The security agent will:
1. Intake scope and identify areas to scan
2. Draft an audit plan
3. Scan for vulnerabilities: injection, broken access control, cryptography, auth issues, misconfigurations, vulnerable dependencies
4. Output a findings report with severity (Critical/High/Medium/Low/Info) and OWASP reference per finding
5. Produce a prioritized fix list
6. Save findings to vault via vault-archivist

Use this skill before shipping auth changes, new API endpoints, file handling, or any feature touching user data.
