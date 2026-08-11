---
title: "misleading backend-selection output printed SQLite path under Postgres"
type: gotcha
---

agentops-cli's install/status commands printed the SQLite graph.db file path unconditionally, even when AGENTOPS_DATABASE_URL selected Postgres as the actual backend — no such SQLite file is ever created in that case, so the output looked like a working feature quietly lying about where data went. Fixed via a shared describe_backend() helper that's backend-aware and redacts the Postgres password before printing a connection string to a terminal.
