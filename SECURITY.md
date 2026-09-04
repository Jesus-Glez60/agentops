# Security Policy

## Reporting a vulnerability

Please report security issues privately by emailing **jagschuy@gmail.com** rather than opening a public issue. Include enough detail to reproduce the problem (affected component, version/commit, and steps). You should get an acknowledgement within a few days.

There is no paid bug bounty program. Please still disclose responsibly and give us a reasonable window to fix an issue before any public disclosure.

**A note on AI-assisted reports**: if a tool helped you find or write up an issue, say so, and verify the finding yourself before submitting. Unverified, AI-generated reports that don't reproduce waste maintainer time and will be closed without much discussion.

## Supply chain notes

The `agentops-cli` npm package (`npm/agentops-cli/`) does not run any script automatically at install time. Its `bin/agentops.js` shim downloads a prebuilt platform binary from this repo's GitHub Releases the first time the `agentops` command is actually invoked, then executes it. If you're auditing this project, that download step (`npm/agentops-cli/bin/agentops.js`) is the relevant supply-chain surface to review.
