---
name: library-docs
description: "Looks up documentation for a library or framework in a strict order: this project's own get_docs/search_docs first, then self-serve ingestion via discover_library/register_library/scrape_library, then a Context7-style MCP tool only if one is available, then training knowledge as a flagged last resort. Use whenever a task needs library/framework documentation and the answer isn't already in hand — including mid-task doc gaps found while planning or coding, not just when docs are explicitly requested."
---

@include ../shared/library-docs.md
