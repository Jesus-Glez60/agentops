---
title: "agentops-heavy-embeddings was renamed but never actually ported"
type: gotcha
---

An earlier session renamed the heavy-tier Qdrant+BGE-M3 embeddings crate from agentops-embeddings to agentops-heavy-embeddings to resolve a naming collision with the new open-core agentops-embeddings crate -- but the rename only touched Cargo.toml/package name, the actual 503-line SemanticIndex implementation from main was never ported, leaving a 16-line stub. This wasn't caught until agentops-heavy-api's port needed to depend on a real SemanticIndex and the type didn't exist. Ported the full implementation as a Phase 1 prerequisite, adapting three places: GraphStore::nodes_by_kind is repo-scoped now (main's wasn't), NodeKind::as_str() is as_db_str() today, and collect_doc_index_items lost its TenantContext parameter since docbrain-graph is single-tenant this rebuild. Lesson: a crate rename is not the same as a port -- verify a renamed stub actually has real content before assuming a dependent crate can build on it.
