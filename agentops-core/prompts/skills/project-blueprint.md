---
name: project-blueprint
description: "Runs a senior-engineer documentation/planning pass producing all 8 standard docs (PRD, Design Brief, Architecture, AI Agents Guide, Data Design, Technical Requirements, Security Guide, App Flow) in both plain-language and ADR-style graph-ingested form. For a new idea, first runs 5 hard-gated sanity checks and refuses generation with the gaps listed if they fail. For an existing codebase, drafts from the scanned repo/graph instead of inventing content, then asks the user to confirm or fill gaps. Use when asked to plan out a new project end-to-end, or to document an existing one properly."
---

@include ../shared/project-blueprint.md
