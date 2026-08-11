---
title: "agentops-embeddings crate naming collision between agentops-core and agentops-heavy"
type: gotcha
---

agentops-heavy already had its own separate crate also named agentops-embeddings (Qdrant + BGE-M3, still a stub) when the new open-core agentops-embeddings crate (BGE-small-en-v1.5 via fastembed, extracted from docbrain-ingest) was added. Same crate name, two entirely different implementations, in two genuinely separate Cargo workspaces so no compile conflict occurred, but real risk of confusion for anyone reading both. Fixed by renaming the heavy-tier one to agentops-heavy-embeddings, with a doc comment explaining why.
