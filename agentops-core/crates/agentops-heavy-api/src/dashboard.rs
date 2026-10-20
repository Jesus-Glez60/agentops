//! Tenant-scoped equivalents of `agentops-api`'s single-operator dashboard
//! routes (`GET /activity`, `GET /local-search`, `GET /gotchas`,
//! `GET /repos/{id}/nodes/{node_id}`, `POST .../curation`,
//! `GET .../graph` x2, `GET /repos/{id}/docs`) -- backing the web
//! dashboard's Overview/Search/Knowledge Graph/Gotchas/Documentation
//! screens for a real tenant connection instead of `agentops-manifest`'s
//! local scan registry (which a hosted, multi-tenant deployment never
//! populates -- see this crate's root doc comment and
//! `agentops-server`'s doc comment on why the two route sets can't both be
//! mounted).
//!
//! Every handler here composes the exact same free functions
//! `agentops-api`'s own handlers do (`agentops_api::repos::summarize_repo`,
//! `agentops_api::search::connected_nodes`, `agentops_api::subgraph::
//! build_subgraph`/`build_repo_graph`, `agentops_api::repos::matches_bucket`,
//! etc. -- see each module's doc comment for why they're `pub`), just fed a
//! `ConnectionStore`-resolved checkout path (via `tenant_repo::
//! resolve_connection_path`) instead of a manifest-derived one. No new
//! graph/search/subgraph logic is written in this file.
//!
//! Every store-touching handler runs its blocking work inside
//! `tokio::task::spawn_blocking` -- `agentops_mcp::open_store` (and
//! anything that reads through it) can `block_on` an internally-owned
//! Tokio runtime when `AGENTOPS_DATABASE_URL` selects `PostgresGraphStore`
//! (see that store's own doc comment); calling it directly from this
//! already-async handler would panic under a Postgres-backed deployment,
//! the exact bug class this codebase has already hit three times (`/mcp`,
//! `agentops-api`'s `/tools/{name}`, and `indexing.rs`'s `run_job`).

use std::path::PathBuf;

use agentops_accounts::User;
use agentops_embeddings::LocalEmbedder;
use agentops_graph::{NodeKind, NodeProminence};
use agentops_manifest::ManifestEntry;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tenant_repo::resolve_connection_path;
use crate::{require_session_capability, resolve_tenant, AppState, TenantQuery};

const DEFAULT_TOP_K: usize = 20;

/// Every `Active` connection for `tenant`, paired with its resolved
/// checkout path and `agentops_mcp::repo_name` -- the shared core every
/// handler below starts from, mirroring `agentops_api::repos::
/// open_scanned_repos`'s "skip anything that doesn't actually open" shape
/// but sourced from `ConnectionStore` instead of the manifest.
fn tenant_repo_paths(state: &AppState, tenant: &str) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let store = state.store.lock().unwrap();
    let connections = store.list_connections(tenant)?;
    Ok(connections
        .into_iter()
        .filter(|c| matches!(c.status, agentops_repo_access::store::ConnectionStatus::Active))
        .map(|c| {
            let path = crate::indexing::checkout_path(&state.repo_checkouts_dir, tenant, &c.id);
            let name = agentops_mcp::repo_name(&path);
            (name, path)
        })
        .collect())
}

async fn tenant_and_capability(state: &AppState, user: &Option<axum::Extension<User>>, provided_tenant: Option<&str>) -> Result<String, (StatusCode, Json<Value>)> {
    let tenant = resolve_tenant(user, provided_tenant)?;
    require_session_capability(state, user, &tenant, agentops_teams::CAP_REPOS_VIEW)?;
    Ok(tenant)
}

/// `GET /activity` -- tenant's connections' recent scan history, same shape
/// as `agentops-api`'s `activity_json`.
pub(crate) async fn activity_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<agentops_api::repos::ActivityEvent>> {
        let repos = tenant_repo_paths(&state, &tenant)?;
        let mut events = Vec::new();
        for (name, path) in repos {
            let Ok(store) = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path) else { continue };
            if let Ok(Some(scan)) = store.latest_scan(&name) {
                events.push(agentops_api::repos::ActivityEvent {
                    repo: name,
                    started_at: scan.started_at,
                    files_added: scan.files_added,
                    files_changed: scan.files_changed,
                    files_removed: scan.files_removed,
                    symbols_added: scan.symbols_added,
                    symbols_changed: scan.symbols_changed,
                    symbols_removed: scan.symbols_removed,
                });
            }
        }
        events.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        events.truncate(20);
        Ok(events)
    })
    .await;

    match result {
        Ok(Ok(events)) => (StatusCode::OK, Json(json!({ "activity": events }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct LocalSearchQuery {
    tenant: Option<String>,
    q: String,
    /// Comma-separated connection ids/URLs; omitted or empty means "every
    /// one of the tenant's own connections" -- never able to reach another
    /// tenant's data even if a caller guesses an id, since resolution below
    /// is always scoped to the caller's own tenant.
    repos: Option<String>,
    kind: Option<String>,
    top_k: Option<usize>,
}

/// `GET /local-search` -- tenant-scoped hybrid search (dense + lexical +
/// exact name, `agentops_retrieval::search_hybrid`, the same ranking MCP
/// `local_semantic_search` uses). Dense-only search found nothing on repos
/// indexed the normal hosted way, since that path stores no embeddings.
/// `similarity` carries `HybridHit::relevance`.
pub(crate) async fn local_search_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, Query(q): Query<LocalSearchQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let top_k = q.top_k.unwrap_or(DEFAULT_TOP_K);
    let repo_filter: Option<Vec<String>> = q.repos.as_deref().map(|s| s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect());
    let kind_filter: Result<Vec<NodeKind>, String> =
        q.kind.as_deref().map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|k| agentops_api::parse_kind(k).ok_or_else(|| k.to_string())).collect()).unwrap_or(Ok(Vec::new()));
    let kinds = match kind_filter {
        Ok(k) => k,
        Err(bad) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'kind': {bad:?}") }))),
    };
    let kind_passes: Vec<Option<NodeKind>> = if kinds.is_empty() { vec![None] } else { kinds.into_iter().map(Some).collect() };
    let query_text = q.q.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<agentops_api::search::SearchResult>> {
        let mut repos = tenant_repo_paths(&state, &tenant)?;
        if let Some(names) = &repo_filter {
            // Filter by either the resolved repo name or the raw connection
            // ref (id/URL) -- resolved separately via resolve_connection_path
            // per requested ref, same tenant-scoped safety property `/mcp`
            // already has (an unresolvable ref for this tenant is just
            // dropped, not treated as a literal name/path).
            let resolved: Vec<String> = names.iter().filter_map(|r| resolve_connection_path(&state, &tenant, r).ok()).map(|p| agentops_mcp::repo_name(&p)).collect();
            repos.retain(|(name, _)| resolved.contains(name));
        }

        let mut hits: Vec<agentops_api::search::SearchResult> = Vec::new();
        for (name, path) in &repos {
            let Ok(store) = agentops_mcp::resolve_store(state.pg_store.as_ref(), path) else { continue };
            for kind in &kind_passes {
                for hit in agentops_retrieval::search_hybrid(store.as_ref(), &LocalEmbedder, name, &query_text, top_k, *kind, false, None)? {
                    let relevance = hit.relevance();
                    let mut result = agentops_api::search::node_to_result(name, hit.node, 0.0);
                    result.similarity = relevance;
                    hits.push(result);
                }
            }
        }
        // Merges the per-repo lists, keeping the same curation demotion
        // `search_hybrid` applies within one repo.
        hits.sort_by(|a, b| {
            let rank_a = a.similarity * agentops_graph::prominence_rank_multiplier(a.prominence) as f32;
            let rank_b = b.similarity * agentops_graph::prominence_rank_multiplier(b.prominence) as f32;
            rank_b.partial_cmp(&rank_a).unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k);
        Ok(hits)
    })
    .await;

    match result {
        Ok(Ok(results)) => (StatusCode::OK, Json(json!({ "results": results }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

/// `GET /repos/{id}/nodes/{node_id}` -- `{id}` resolved the same way
/// `/repos/{id}/verify` etc. already are.
pub(crate) async fn node_detail_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath((id, node_id)): AxumPath<(String, i64)>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<agentops_api::search::NodeDetail>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        let Some(node) = store.get_node(&repo, node_id)? else { return Ok(None) };
        let connected = agentops_api::search::connected_nodes(store.as_ref(), &repo, node_id)?;
        Ok(Some(agentops_api::search::NodeDetail {
            id: node.id,
            kind: node.kind,
            repo,
            path: node.path,
            name: node.name,
            container: node.container,
            start_line: node.start_line,
            end_line: node.end_line,
            content: node.content,
            curated: node.curated,
            prominence: node.prominence,
            curation_reason: node.curation_reason,
            connected,
        }))
    })
    .await;

    match result {
        Ok(Ok(Some(detail))) => (StatusCode::OK, Json(serde_json::to_value(detail).unwrap())),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such node" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Deserialize)]
pub(crate) struct SimilarityQuery {
    #[serde(default)]
    tenant: Option<String>,
    /// Comma-separated node ids. `#[serde(default)]` for the same reason
    /// as `TenantQuery::tenant`: an absent query string must not 400.
    #[serde(default)]
    to: String,
}

/// `GET /repos/{id}/nodes/{node_id}/similarity?to=a,b` -- the cosine
/// between `node_id`'s stored vector and each listed node's, read from the
/// store (no model is loaded). `SimilarTo` edges keep no score, so this is
/// how `agentops recall-eval --remote` tells an equivalent memory from a
/// miss. A listed id that doesn't exist or has no vector is left out of
/// `scores`, as is everything when `node_id` itself has none.
pub(crate) async fn node_similarity_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath((id, node_id)): AxumPath<(String, i64)>, Query(q): Query<SimilarityQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let to: Vec<i64> = q.to.split(',').filter_map(|s| s.trim().parse().ok()).collect();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<serde_json::Map<String, Value>>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        let mut scores = serde_json::Map::new();
        let Some(source) = store.get_embedding(&repo, node_id)? else { return Ok(Some(scores)) };
        for other in to {
            if let Some(vector) = store.get_embedding(&repo, other)? {
                scores.insert(other.to_string(), json!(agentops_graph::similarity::cosine(&source, &vector)));
            }
        }
        Ok(Some(scores))
    })
    .await;

    match result {
        Ok(Ok(Some(scores))) => (StatusCode::OK, Json(json!({ "scores": scores }))),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo connection" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

/// Writes one curation action to the tenant's audit log (milestone 2, ADR
/// 0050: every curation is logged, not only team actions). The actor is
/// the session's or personal API key's user; the instance-wide key has
/// none, so the row's actor is NULL and `via` says so. Best effort: no
/// teams store, or a failed write, never fails the curation itself.
fn audit_curation(state: &AppState, tenant: &str, user: &Option<axum::Extension<User>>, action: &str, target: &str, mut metadata: Value) {
    let Some(teams) = &state.teams else {
        eprintln!("agentops: no teams store, so {action} on {target} isn't audit-logged");
        return;
    };
    let actor = user.as_ref().map(|axum::Extension(u)| u.id);
    if actor.is_none() {
        metadata["via"] = json!("instance_api_key");
    }
    if let Err(e) = teams.lock().unwrap().record_audit_event(tenant, actor, action, Some(target), None, &metadata.to_string()) {
        eprintln!("agentops: audit log write for {action} on {target} failed: {e:#}");
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetNodeKindBody {
    /// `gotcha`, `decision` or `knowledge` (a `Note`).
    kind: String,
}

/// `POST /repos/{id}/nodes/{node_id}/kind` -- the Gotchas workspace's "set
/// kind" on a memory, typically a `NeedsReview` one the importer couldn't
/// classify. Changes the node's kind in place and rewrites its note file's
/// frontmatter `type:` in the connection's notes folder (ADR 0045), or the
/// next reindex would ingest the file as a second node of the old kind.
/// Only memory nodes (gotcha, decision, note) can change kind.
pub(crate) async fn set_node_kind_json(
    State(state): State<AppState>,
    user: Option<axum::Extension<User>>,
    AxumPath((id, node_id)): AxumPath<(String, i64)>,
    Query(q): Query<TenantQuery>,
    Json(body): Json<SetNodeKindBody>,
) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let (kind, note_type) = match body.kind.as_str() {
        "gotcha" => (NodeKind::Gotcha, "gotcha"),
        "decision" => (NodeKind::Decision, "decision"),
        "knowledge" => (NodeKind::Note, "knowledge"),
        other => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'kind': {other:?} (expected gotcha, decision or knowledge)") }))),
    };

    let audit_state = state.clone();
    let audit_tenant = tenant.clone();
    let audit_target = format!("{id}/nodes/{node_id}");
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Result<(), (StatusCode, String)>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(Err((StatusCode::NOT_FOUND, "no such node".into()))),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        let Some(node) = store.get_node(&repo, node_id)? else {
            return Ok(Err((StatusCode::NOT_FOUND, "no such node".into())));
        };
        if !matches!(node.kind, NodeKind::Gotcha | NodeKind::Decision | NodeKind::Note) {
            return Ok(Err((StatusCode::BAD_REQUEST, format!("only gotchas, decisions and notes change kind, not a {}", node.kind.as_db_str()))));
        }
        if let Some(rel) = node.path.as_deref().and_then(|p| p.strip_prefix("vault:")) {
            let file = crate::indexing::notes_dir_for_checkout(&path).join(rel);
            if let Ok(content) = std::fs::read_to_string(&file) {
                std::fs::write(&file, agentops_notes::with_frontmatter_type(&content, note_type))?;
            }
        }
        store.set_node_kind(&repo, node_id, kind)?;
        Ok(Ok(()))
    })
    .await;

    match result {
        Ok(Ok(Ok(()))) => {
            audit_curation(&audit_state, &audit_tenant, &user, "node_kind_set", &audit_target, json!({ "kind": note_type }));
            (StatusCode::OK, Json(json!({ "id": node_id, "kind": note_type })))
        }
        Ok(Ok(Err((status, error)))) => (status, Json(json!({ "error": error }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetCurationBody {
    prominence: String,
    reason: Option<String>,
}

fn parse_prominence(s: &str) -> Option<NodeProminence> {
    match s {
        "full" => Some(NodeProminence::Full),
        "reduced" => Some(NodeProminence::Reduced),
        "pinned" => Some(NodeProminence::Pinned),
        "needs_review" => Some(NodeProminence::NeedsReview),
        _ => None,
    }
}

/// `POST /repos/{id}/nodes/{node_id}/curation` -- path resolution mirrors
/// `node_detail_json` exactly. Gated only by `repos.view` (via
/// `tenant_and_capability`), on purpose: any authorized member may curate,
/// since the knowledge comes from working on the codebase (author decision,
/// 2026-10-13). `gotchas.resolve` in the permissions matrix is not checked.
pub(crate) async fn set_curation_json(
    State(state): State<AppState>,
    user: Option<axum::Extension<User>>,
    AxumPath((id, node_id)): AxumPath<(String, i64)>,
    Query(q): Query<TenantQuery>,
    Json(body): Json<SetCurationBody>,
) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let Some(prominence) = parse_prominence(&body.prominence) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'prominence': {:?}", body.prominence) })));
    };
    let reason = body.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
    if prominence.requires_reason() && reason.is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "'reason' is required (non-empty) when prominence is 'reduced' or 'needs_review'" })));
    }
    let reason = reason.map(str::to_string);

    let audit_state = state.clone();
    let audit_tenant = tenant.clone();
    let audit_target = format!("{id}/nodes/{node_id}");
    let audit_metadata = json!({ "prominence": prominence.as_db_str(), "reason": reason });
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<()>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        if store.get_node(&repo, node_id)?.is_none() {
            return Ok(None);
        }
        store.set_curation(&repo, node_id, prominence, reason.as_deref())?;
        Ok(Some(()))
    })
    .await;

    if matches!(result, Ok(Ok(Some(())))) {
        audit_curation(&audit_state, &audit_tenant, &user, "node_curated", &audit_target, audit_metadata);
    }
    match result {
        Ok(Ok(Some(()))) => (StatusCode::OK, Json(json!({ "id": node_id, "prominence": prominence }))),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such node" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubgraphQuery {
    tenant: Option<String>,
    mode: String,
    depth: Option<u32>,
    kind: Option<String>,
}

/// `GET /repos/{id}/nodes/{node_id}/graph` -- reuses
/// `agentops_api::subgraph::build_subgraph` unchanged.
pub(crate) async fn subgraph_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath((id, node_id)): AxumPath<(String, i64)>, Query(q): Query<SubgraphQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let depth = q.depth.unwrap_or(2).clamp(1, 4);
    let kind_filter: Result<Vec<NodeKind>, String> =
        q.kind.as_deref().map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|k| agentops_api::parse_kind(k).ok_or_else(|| k.to_string())).collect()).unwrap_or(Ok(Vec::new()));
    let kinds = match kind_filter {
        Ok(k) => k,
        Err(bad) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'kind': {bad:?}") }))),
    };
    let mode = q.mode.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<agentops_api::subgraph::SubgraphResponse>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        agentops_api::subgraph::build_subgraph(store.as_ref(), &repo, node_id, &mode, depth, &kinds)
    })
    .await;

    match result {
        Ok(Ok(Some(subgraph))) => (StatusCode::OK, Json(serde_json::to_value(subgraph).unwrap())),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such node, or invalid mode" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RepoGraphQuery {
    tenant: Option<String>,
    kind: Option<String>,
}

/// `GET /repos/{id}/graph` -- reuses `agentops_api::subgraph::build_repo_graph`.
pub(crate) async fn repo_graph_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<RepoGraphQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let kind_filter: Result<Vec<NodeKind>, String> =
        q.kind.as_deref().map(|s| s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|k| agentops_api::parse_kind(k).ok_or_else(|| k.to_string())).collect()).unwrap_or(Ok(Vec::new()));
    let kinds = match kind_filter {
        Ok(k) => k,
        Err(bad) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'kind': {bad:?}") }))),
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<agentops_api::subgraph::RepoGraphResponse>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        Ok(Some(agentops_api::subgraph::build_repo_graph(store.as_ref(), &repo, &kinds)?))
    })
    .await;

    match result {
        Ok(Ok(Some(graph))) => (StatusCode::OK, Json(serde_json::to_value(graph).unwrap())),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo connection" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

/// `GET /repos/{id}/usage` -- Module 8's usage/knowledge-reuse dashboard
/// card, reusing `agentops_api::usage::usage_summary` exactly (see that
/// module's doc comment for the aggregation/estimate approach).
pub(crate) async fn usage_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<agentops_api::usage::UsageSummary>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        Ok(Some(agentops_api::usage::usage_summary(store.as_ref(), &repo)?))
    })
    .await;

    match result {
        Ok(Ok(Some(summary))) => (StatusCode::OK, Json(serde_json::to_value(summary).unwrap())),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo connection" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageSyncEntry {
    session_id: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_estimate_usd: f64,
    /// Absent from CLIs older than this field -- defaults to 0 ("unknown").
    #[serde(default)]
    peak_context_tokens: i64,
    session_started_at: String,
    session_ended_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageSyncBody {
    entries: Vec<UsageSyncEntry>,
    /// Per-session token breakdowns; absent from CLIs older than this field.
    #[serde(default)]
    reports: Vec<agentops_graph::NewSessionReport>,
}

/// `POST /repos/{id}/usage/sync` -- the hosted counterpart to
/// `agentops-cli usage sync`'s local-store path (`GraphStore::
/// upsert_session_usage` directly): a repo connected via `agentops connect
/// --remote` has no local graph store worth writing to, so the CLI pushes
/// its parsed Claude Code JSONL entries here instead, authenticated with
/// the same personal API key `connect_remote`'s device-login/`--api-key`
/// flow already obtained (see `require_api_key_or_session`, which this
/// router's blanket `.layer()` already applies -- no `/mcp`-only
/// `require_tenant_auth` needed here, since a personal API key is exactly
/// what that middleware also accepts).
pub(crate) async fn usage_sync_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<TenantQuery>, Json(body): Json<UsageSyncBody>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<usize>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        for entry in &body.entries {
            store.upsert_session_usage(agentops_graph::NewSessionUsage {
                repo: repo.clone(),
                session_id: entry.session_id.clone(),
                model: entry.model.clone(),
                input_tokens: entry.input_tokens,
                output_tokens: entry.output_tokens,
                cache_read_tokens: entry.cache_read_tokens,
                cache_write_tokens: entry.cache_write_tokens,
                cost_estimate_usd: entry.cost_estimate_usd,
                peak_context_tokens: entry.peak_context_tokens,
                session_started_at: entry.session_started_at.clone(),
                session_ended_at: entry.session_ended_at.clone(),
            })?;
        }
        for report in &body.reports {
            // `repo` comes from the resolved connection, never the wire.
            store.upsert_session_report(agentops_graph::NewSessionReport { repo: repo.clone(), ..report.clone() })?;
        }
        Ok(Some(body.entries.len()))
    })
    .await;

    match result {
        Ok(Ok(Some(synced))) => (StatusCode::OK, Json(json!({ "synced": synced }))),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo connection" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

/// The embedder the server's hooks recall by meaning with. The model stays
/// loaded in this long-running process (warmed at startup by
/// `agentops-server`), unlike a local hook process, which never loads it
/// (ADR 0052).
static SERVER_EMBEDDER: LocalEmbedder = LocalEmbedder;

/// `POST /repos/{id}/hooks/{event}` -- the hosted half of `agentops
/// hook-capture`. A repo connected via `agentops connect --remote` forwards
/// each Claude Code hook payload here instead of opening a local graph, and
/// this runs the exact function the local path runs
/// (`agentops_mcp::hooks::handle`) against the connection's store, so the
/// two can't drift. The repo comes from the resolved connection, never from
/// the payload. Replies `{"output": <hook stdout JSON or null>}`. Same
/// personal-API-key auth as `usage_sync_json`.
pub(crate) async fn hook_event_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath((id, event)): AxumPath<(String, String)>, Query(q): Query<TenantQuery>, Json(payload): Json<Value>) -> (StatusCode, Json<Value>) {
    if !agentops_mcp::hooks::STORE_EVENTS.contains(&event.as_str()) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("unknown hook event '{event}'") })));
    }
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Option<Value>>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        Ok(Some(agentops_mcp::hooks::handle_with(store.as_ref(), &repo, &event, &payload, Some(&SERVER_EMBEDDER))?))
    })
    .await;

    match result {
        Ok(Ok(Some(output))) => (StatusCode::OK, Json(json!({ "output": output }))),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo connection" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Deserialize)]
pub(crate) struct SetNodeTitleBody {
    title: String,
}

/// `POST /repos/{id}/nodes/{node_id}/title` -- renames a gotcha, decision
/// or note in place through `agentops_mcp::retitle_note` (id, edges, tags,
/// curation and vector kept; its note file in the checkout moves too).
/// `agentops import-memory --retitle` uses it to replace slug titles with
/// descriptions. Audit-logged like every curation action.
pub(crate) async fn set_node_title_json(
    State(state): State<AppState>,
    user: Option<axum::Extension<User>>,
    AxumPath((id, node_id)): AxumPath<(String, i64)>,
    Query(q): Query<TenantQuery>,
    Json(body): Json<SetNodeTitleBody>,
) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let title = body.title.trim().to_string();
    if title.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "'title' can't be empty" })));
    }

    let audit_state = state.clone();
    let audit_tenant = tenant.clone();
    let audit_target = format!("{id}/nodes/{node_id}");
    let new_title = title.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Result<(Option<String>, bool), (StatusCode, String)>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(Err((StatusCode::NOT_FOUND, "no such node".into()))),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        let Some(node) = store.get_node(&repo, node_id)? else {
            return Ok(Err((StatusCode::NOT_FOUND, "no such node".into())));
        };
        if !matches!(node.kind, NodeKind::Gotcha | NodeKind::Decision | NodeKind::Note) {
            return Ok(Err((StatusCode::BAD_REQUEST, format!("only gotchas, decisions and notes are retitled, not a {}", node.kind.as_db_str()))));
        }
        match agentops_mcp::retitle_note(store.as_ref(), &repo, &crate::indexing::notes_dir_for_checkout(&path), node_id, &title) {
            Ok(changed) => Ok(Ok((node.name, changed))),
            Err(e) => Ok(Err((StatusCode::CONFLICT, e.to_string()))),
        }
    })
    .await;

    match result {
        Ok(Ok(Ok((old, changed)))) => {
            if changed {
                audit_curation(&audit_state, &audit_tenant, &user, "node_retitled", &audit_target, json!({ "from": old, "to": new_title }));
            }
            (StatusCode::OK, Json(json!({ "id": node_id, "title": new_title, "changed": changed })))
        }
        Ok(Ok(Err((status, error)))) => (status, Json(json!({ "error": error }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

/// Most pointers `POST /repos/{id}/recall` returns.
const RECALL_DEEP_MAX: usize = 50;

#[derive(Deserialize)]
pub(crate) struct RecallBody {
    prompt: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// `POST /repos/{id}/recall` -- the ranked recall pointers for `prompt`
/// from the same retrieval call the prompt hook makes, but deeper than a
/// seed (up to `RECALL_DEEP_MAX`) and without recording anything in the
/// session. `agentops recall-eval` uses it to tell a memory ranked just
/// below the seed from one recall never finds.
pub(crate) async fn recall_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<TenantQuery>, Json(body): Json<RecallBody>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let limit = body.limit.unwrap_or(RECALL_DEEP_MAX).min(RECALL_DEEP_MAX);

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<Value>>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        let pointers = agentops_retrieval::recall_seed_with(store.as_ref(), &repo, &body.prompt, limit, &Default::default(), Some(&SERVER_EMBEDDER))?;
        Ok(Some(pointers.into_iter().map(|p| json!({ "node_id": p.node_id, "kind": p.kind.as_db_str(), "title": p.title, "reason": p.reason })).collect()))
    })
    .await;

    match result {
        Ok(Ok(Some(pointers))) => (StatusCode::OK, Json(json!({ "pointers": pointers }))),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo connection" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Deserialize)]
pub(crate) struct WorkRecallBody {
    cue: agentops_retrieval::work_recall::WorkCue,
    #[serde(default)]
    sources: agentops_retrieval::work_recall::CueSources,
    #[serde(default)]
    limit: Option<usize>,
}

/// `POST /repos/{id}/work-recall` -- the candidates one work cue (a file or
/// an error) reaches, ranked, from the same resolution the `PostToolUse`
/// hook runs (`agentops_retrieval::work_recall::resolve_cue`), up to
/// `RECALL_DEEP_MAX`, recording nothing. `agentops work-recall-eval` scores
/// its cases against it.
pub(crate) async fn work_recall_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<TenantQuery>, Json(body): Json<WorkRecallBody>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let limit = body.limit.unwrap_or(RECALL_DEEP_MAX).min(RECALL_DEEP_MAX);

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<Value>>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let repo = agentops_mcp::repo_name(&path);
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        let candidates = agentops_retrieval::work_recall::resolve_cue(store.as_ref(), &repo, &body.cue, body.sources)?;
        Ok(Some(candidates.into_iter().take(limit).map(|c| json!({ "node_id": c.pointer.node_id, "kind": c.pointer.kind.as_db_str(), "title": c.pointer.title, "reason": c.pointer.reason, "score": c.score })).collect()))
    })
    .await;

    match result {
        Ok(Ok(Some(candidates))) => (StatusCode::OK, Json(json!({ "candidates": candidates }))),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo connection" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

/// `GET /repos/{id}/docs` -- mirrors `agentops-api`'s `docs_json` cache-hit/
/// miss logic exactly (persisted `doc_pages` row if present, else a
/// heuristic-only page built on the fly, never persisted from here).
pub(crate) async fn docs_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, AxumPath(id): AxumPath<String>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Value>> {
        let path = match resolve_connection_path(&state, &tenant, &id) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        let store = agentops_mcp::resolve_store(state.pg_store.as_ref(), &path)?;
        let repo = agentops_mcp::repo_name(&path);

        if let Some((_generated_at, content_json)) = store.get_doc_page(&repo)? {
            let value: Value = serde_json::from_str(&content_json)?;
            return Ok(Some(value));
        }
        if store.latest_scan(&repo)?.is_none() {
            return Ok(None);
        }
        let report = agentops_scanner::scan_repo(&path)?;
        let ranked: Vec<PathBuf> = agentops_scanner::rank_files(&path, &report.files).into_iter().map(|(p, _)| p).collect();
        let doc_page = agentops_docgen::build_doc_page(store.as_ref(), &repo, &ranked, &[])?;
        Ok(Some(serde_json::to_value(doc_page)?))
    })
    .await;

    match result {
        Ok(Ok(Some(doc_page))) => (StatusCode::OK, Json(doc_page)),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such repo connection, or it has not been scanned yet" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct GotchasQuery {
    tenant: Option<String>,
    bucket: Option<String>,
    /// Comma-separated connection ids/URLs; omitted or empty means every
    /// one of the tenant's own connections -- same resolution/safety
    /// property as `LocalSearchQuery::repos`.
    repos: Option<String>,
}

/// `GET /gotchas` -- tenant-scoped, iterating the tenant's own connections
/// instead of the manifest; reuses `agentops_api::repos::matches_bucket`/
/// `GOTCHA_BUCKETS` and `agentops_api::search::snippet`.
pub(crate) async fn gotchas_json(State(state): State<AppState>, user: Option<axum::Extension<User>>, Query(q): Query<GotchasQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let bucket = q.bucket.clone();
    if let Some(b) = &bucket {
        if !agentops_api::repos::GOTCHA_BUCKETS.contains(&b.as_str()) {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid 'bucket': {b:?}") })));
        }
    }
    let repo_filter: Option<Vec<String>> = q.repos.as_deref().map(|s| s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect());

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<agentops_api::repos::GotchaSummary>> {
        let mut repos = tenant_repo_paths(&state, &tenant)?;
        if let Some(names) = &repo_filter {
            let resolved: Vec<String> = names.iter().filter_map(|r| resolve_connection_path(&state, &tenant, r).ok()).map(|p| agentops_mcp::repo_name(&p)).collect();
            repos.retain(|(name, _)| resolved.contains(name));
        }

        let mut gotchas = Vec::new();
        for (name, path) in &repos {
            let Ok(store) = agentops_mcp::resolve_store(state.pg_store.as_ref(), path) else { continue };
            for node in agentops_api::repos::bucket_kinds(bucket.as_deref()).iter().map(|&kind| store.nodes_by_kind(name, kind)).collect::<anyhow::Result<Vec<_>>>()?.into_iter().flatten() {
                if let Some(b) = &bucket {
                    if !agentops_api::repos::matches_bucket(&node, b) {
                        continue;
                    }
                }
                gotchas.push(agentops_api::repos::GotchaSummary {
                    repo: name.clone(),
                    id: node.id,
                    name: node.name,
                    path: node.path,
                    container: node.container,
                    start_line: node.start_line,
                    end_line: node.end_line,
                    snippet: agentops_api::search::snippet(&node.content),
                    curated: node.curated,
                    prominence: node.prominence,
                    curation_reason: node.curation_reason,
                    kind: node.kind,
                });
            }
        }
        Ok(gotchas)
    })
    .await;

    match result {
        Ok(Ok(gotchas)) => (StatusCode::OK, Json(json!({ "gotchas": gotchas }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

/// Extends `GET /repos`'s `ConnectionView` with real counts for `Active`
/// connections -- reuses `agentops_api::repos::summarize_repo` fed a
/// synthetic `ManifestEntry` built from the resolved checkout path (see
/// `agentops_api::repos::summarize_repo`'s doc comment for why this is
/// legitimate reuse, not a hack). Returns `(None, None, false)` for a
/// non-`Active` connection or one whose store fails to open -- never
/// fabricated zeros, matching `RepoSummary.counts`'s own `None`-means-
/// "never scanned" contract.
pub(crate) fn connection_counts(state: &AppState, tenant: &str, connection_id: &str, active: bool) -> (Option<agentops_api::repos::RepoCounts>, Option<String>, bool) {
    if !active {
        return (None, None, false);
    }
    let path = crate::indexing::checkout_path(&state.repo_checkouts_dir, tenant, connection_id);
    let entry = ManifestEntry { path: path.display().to_string(), last_scanned_at: 0 };
    let summary = agentops_api::repos::summarize_repo(&entry, state.pg_store.as_ref());
    (summary.counts, summary.branch, summary.path_missing)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use agentops_repo_access::secrets::EnvSecretsProvider;
    use agentops_repo_access::store::ConnectionStore;
    use std::sync::Arc;

    fn test_state() -> (ConnectionStore, Arc<dyn agentops_repo_access::secrets::SecretsProvider + Send + Sync>) {
        let store = ConnectionStore::open_in_memory().unwrap();
        let secrets: Arc<dyn agentops_repo_access::secrets::SecretsProvider + Send + Sync> = Arc::new(EnvSecretsProvider::from_hex(&"22".repeat(32)).unwrap());
        (store, secrets)
    }

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn signup(accounts: &agentops_accounts::AccountStore, email: &str) -> (agentops_accounts::User, String) {
        accounts.signup(agentops_accounts::NewAccount { email, password: "correct horse battery staple", first_name: "Ada", last_name: "Lovelace" }).unwrap()
    }

    /// Creates an `Active` connection whose checkout directory is real,
    /// scanned data (a genuine `agentops_mcp::scan_and_persist` call against
    /// `checkout_path(...)`) -- simulates a completed indexing job without
    /// running the full clone pipeline, same shortcut this crate's own
    /// `indexing.rs` tests aren't needed for since only the store-reading
    /// side is under test here.
    fn seed_active_indexed_connection(checkouts_dir: &std::path::Path, store: &ConnectionStore, secrets: &(dyn agentops_repo_access::secrets::SecretsProvider + Send + Sync), tenant: &str, repo_id: &str) -> String {
        let keypair = agentops_repo_access::generate_deploy_keypair_for_repo(secrets, tenant, repo_id).unwrap();
        let connection = store.create_ssh_connection(tenant, repo_id, "git@github.com:acme/widgets.git", &keypair).unwrap();
        store.set_status(tenant, &connection.id, agentops_repo_access::store::ConnectionStatus::Active).unwrap();

        let path = crate::indexing::checkout_path(checkouts_dir, tenant, &connection.id);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        agentops_mcp::scan_and_persist(&path, false).unwrap();
        connection.id
    }

    #[tokio::test]
    async fn get_repos_reports_real_counts_for_an_active_indexed_connection() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (user, token) = signup(&accounts, "dev@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&user.tenant, user.id, "admin").unwrap();

        seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &user.tenant, "widgets");

        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);

        let resp = app.oneshot(Request::builder().uri("/repos").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = body_json(resp).await;
        let connections = body["connections"].as_array().unwrap();
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0]["counts"]["files"], 1, "{body:?}");
        assert_eq!(connections[0]["counts"]["symbols"], 1, "{body:?}");
        assert_eq!(connections[0]["path_missing"], false);
    }

    // Hosted indexing persists without embeddings, so the old dense-only
    // search returned nothing for a symbol the graph plainly has.
    #[tokio::test]
    async fn local_search_finds_a_symbol_by_name_in_a_repo_indexed_without_embeddings() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (user, token) = signup(&accounts, "dev@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&user.tenant, user.id, "admin").unwrap();

        seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &user.tenant, "widgets");

        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);

        let resp = app.oneshot(Request::builder().uri("/local-search?q=greet").header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = body_json(resp).await;
        let results = body["results"].as_array().unwrap();
        assert!(results.iter().any(|r| r["name"] == "greet"), "{body:?}");
        let similarity = results[0]["similarity"].as_f64().unwrap();
        assert!(similarity > 0.0 && similarity <= 1.0, "similarity must stay a 0..1 relevance: {body:?}");
    }

    #[tokio::test]
    async fn gotchas_never_cross_tenants() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let (other, other_token) = signup(&accounts, "other@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&other.tenant, other.id, "admin").unwrap();

        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");
        let path = crate::indexing::checkout_path(checkouts_dir.path(), &owner.tenant, &connection_id);
        let notes_dir = path.join(".agentops").join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("gotcha.md"), "---\ntitle: \"A real gotcha\"\ntype: gotcha\n---\n\nSomething tricky.\n").unwrap();
        agentops_mcp::scan_and_persist(&path, false).unwrap();

        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);

        let owner_resp = app.clone().oneshot(Request::builder().uri("/gotchas").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let owner_body = body_json(owner_resp).await;
        assert_eq!(owner_body["gotchas"].as_array().unwrap().len(), 1, "{owner_body:?}");

        let other_resp = app.oneshot(Request::builder().uri("/gotchas").header("authorization", format!("Bearer {other_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let other_body = body_json(other_resp).await;
        assert_eq!(other_body["gotchas"].as_array().unwrap().len(), 0, "a different tenant must never see this tenant's gotchas: {other_body:?}");
    }

    #[tokio::test]
    async fn node_detail_404s_when_a_different_tenant_requests_another_tenants_connection_id() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let (other, other_token) = signup(&accounts, "other@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&other.tenant, other.id, "admin").unwrap();

        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");
        let path = crate::indexing::checkout_path(checkouts_dir.path(), &owner.tenant, &connection_id);
        let repo = agentops_mcp::repo_name(&path);
        let node_store = agentops_mcp::open_store(&path).unwrap();
        let node_id = node_store.nodes_by_kind(&repo, agentops_graph::NodeKind::Symbol).unwrap()[0].id;

        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);

        let owner_resp = app
            .clone()
            .oneshot(Request::builder().uri(format!("/repos/{connection_id}/nodes/{node_id}")).header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(owner_resp.status(), axum::http::StatusCode::OK);

        let other_resp = app
            .oneshot(Request::builder().uri(format!("/repos/{connection_id}/nodes/{node_id}")).header("authorization", format!("Bearer {other_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(other_resp.status(), axum::http::StatusCode::NOT_FOUND, "a different tenant must never resolve another tenant's connection id");
    }

    /// Live bug, 2026-10-09: /gotchas labels each gotcha with its store
    /// repo name (`<id>--<tenant>`), and the Gotchas screen posts curation
    /// with that -- every one 404'd ("153 of 153 couldn't be updated").
    #[tokio::test]
    async fn curation_accepts_the_store_repo_name_gotchas_lists_but_never_another_tenants() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let (other, other_token) = signup(&accounts, "other@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&other.tenant, other.id, "admin").unwrap();
        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");

        let path = crate::indexing::checkout_path(checkouts_dir.path(), &owner.tenant, &connection_id);
        let repo = agentops_mcp::repo_name(&path);
        assert_eq!(repo, format!("{connection_id}--{}", owner.tenant));
        let node_id = agentops_mcp::open_store(&path)
            .unwrap()
            .add_node(agentops_graph::NewNode { kind: agentops_graph::NodeKind::Gotcha, repo: repo.clone(), path: None, name: Some("g".into()), container: None, start_line: None, end_line: None, content: Some("body".into()) })
            .unwrap();

        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);
        let post = |token: &str| {
            Request::builder()
                .method("POST")
                .uri(format!("/repos/{repo}/nodes/{node_id}/curation"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "prominence": "full", "reason": null }).to_string()))
                .unwrap()
        };
        let resp = app.clone().oneshot(post(&owner_token)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let resp = app.oneshot(post(&other_token)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND, "another tenant must never resolve the owner's store name");
    }

    /// Milestone 2 item 1 end to end: a memory goes to `needs_review` (a
    /// reason is required), shows up in that bucket whatever its kind, gets
    /// its kind set (node and note file both), and every step is in the
    /// tenant's audit log with the acting user.
    #[tokio::test]
    async fn needs_review_is_listed_across_kinds_its_kind_can_be_set_and_every_curation_is_audited() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let teams_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let teams = agentops_teams::TeamStore::open(&teams_dir.path().join("teams.db")).unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");
        let path = crate::indexing::checkout_path(checkouts_dir.path(), &owner.tenant, &connection_id);
        let repo = agentops_mcp::repo_name(&path);
        let notes_dir = crate::indexing::notes_dir_for_checkout(&path);
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("m.md"), "---\ntitle: \"m\"\ntype: knowledge\n---\n\nbody\n").unwrap();
        let node_id = agentops_mcp::open_store(&path)
            .unwrap()
            .add_node(agentops_graph::NewNode { kind: agentops_graph::NodeKind::Note, repo: repo.clone(), path: Some("vault:m.md".into()), name: Some("m".into()), container: None, start_line: None, end_line: None, content: Some("body".into()) })
            .unwrap();

        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);
        let post = |uri: String, body: Value| Request::builder().method("POST").uri(uri).header("authorization", format!("Bearer {owner_token}")).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let curation = format!("/repos/{repo}/nodes/{node_id}/curation");

        let no_reason = app.clone().oneshot(post(curation.clone(), json!({ "prominence": "needs_review", "reason": " " }))).await.unwrap();
        assert_eq!(no_reason.status(), axum::http::StatusCode::BAD_REQUEST);
        let ok = app.clone().oneshot(post(curation.clone(), json!({ "prominence": "needs_review", "reason": "unclassified on import" }))).await.unwrap();
        assert_eq!(ok.status(), axum::http::StatusCode::OK);

        let listed = app.clone().oneshot(Request::builder().uri("/gotchas?bucket=needs_review").header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap()).await.unwrap();
        let listed = body_json(listed).await;
        let entry = listed["gotchas"].as_array().unwrap().iter().find(|g| g["id"] == node_id).unwrap_or_else(|| panic!("{listed}")).clone();
        assert_eq!((entry["kind"].clone(), entry["prominence"].clone(), entry["curated"].clone()), (json!("Note"), json!("NeedsReview"), json!(false)));

        let bad_kind = app.clone().oneshot(post(format!("/repos/{repo}/nodes/{node_id}/kind"), json!({ "kind": "symbol" }))).await.unwrap();
        assert_eq!(bad_kind.status(), axum::http::StatusCode::BAD_REQUEST);
        let set_kind = app.clone().oneshot(post(format!("/repos/{repo}/nodes/{node_id}/kind"), json!({ "kind": "gotcha" }))).await.unwrap();
        assert_eq!(set_kind.status(), axum::http::StatusCode::OK);
        assert_eq!(agentops_mcp::open_store(&path).unwrap().get_node(&repo, node_id).unwrap().unwrap().kind, agentops_graph::NodeKind::Gotcha);
        assert!(std::fs::read_to_string(notes_dir.join("m.md")).unwrap().contains("type: gotcha"));
        let accepted = app.oneshot(post(curation, json!({ "prominence": "full", "reason": null }))).await.unwrap();
        assert_eq!(accepted.status(), axum::http::StatusCode::OK);

        let audit = agentops_teams::TeamStore::open(&teams_dir.path().join("teams.db")).unwrap().list_audit_log(&owner.tenant, None, None, None).unwrap();
        let mut actions: Vec<(String, Option<i64>)> = audit.iter().map(|e| (e.action.clone(), e.actor_user_id)).collect();
        actions.sort();
        assert_eq!(actions, vec![("node_curated".to_string(), Some(owner.id)), ("node_curated".to_string(), Some(owner.id)), ("node_kind_set".to_string(), Some(owner.id))]);
        assert!(audit.iter().any(|e| e.metadata.contains("needs_review") && e.metadata.contains("unclassified on import")), "{audit:?}");
    }

    #[tokio::test]
    async fn a_retitle_renames_the_node_and_its_file_refuses_a_taken_title_and_is_audited() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let teams_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let teams = agentops_teams::TeamStore::open(&teams_dir.path().join("teams.db")).unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");
        let path = crate::indexing::checkout_path(checkouts_dir.path(), &owner.tenant, &connection_id);
        let repo = agentops_mcp::repo_name(&path);
        let notes_dir = crate::indexing::notes_dir_for_checkout(&path);
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("project-x.md"), "---\ntitle: \"project-x\"\ntype: knowledge\n---\n\nbody\n").unwrap();
        let (node_id, taken) = {
            let graph = agentops_mcp::open_store(&path).unwrap();
            let add = |p: &str, n: &str| graph.add_node(agentops_graph::NewNode { kind: agentops_graph::NodeKind::Note, repo: repo.clone(), path: Some(p.into()), name: Some(n.into()), container: None, start_line: None, end_line: None, content: Some("body".into()) }).unwrap();
            (add("vault:project-x.md", "project-x"), add("vault:already-here.md", "Already here"))
        };

        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);
        let post = |uri: String, body: Value| Request::builder().method("POST").uri(uri).header("authorization", format!("Bearer {owner_token}")).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let title = |id: i64| format!("/repos/{repo}/nodes/{id}/title");

        let ok = app.clone().oneshot(post(title(node_id), json!({ "title": "Project X shipped its first release" }))).await.unwrap();
        assert_eq!(ok.status(), axum::http::StatusCode::OK);
        let node = agentops_mcp::open_store(&path).unwrap().get_node(&repo, node_id).unwrap().unwrap();
        assert_eq!((node.name.as_deref(), node.path.as_deref()), (Some("Project X shipped its first release"), Some("vault:project-x-shipped-its-first-release.md")));
        assert!(!notes_dir.join("project-x.md").exists());
        assert!(std::fs::read_to_string(notes_dir.join("project-x-shipped-its-first-release.md")).unwrap().contains("title: \"Project X shipped its first release\""));

        let clash = app.clone().oneshot(post(title(node_id), json!({ "title": "Already here" }))).await.unwrap();
        assert_eq!(clash.status(), axum::http::StatusCode::CONFLICT, "node {taken} holds that title");
        let empty = app.clone().oneshot(post(title(node_id), json!({ "title": "  " }))).await.unwrap();
        assert_eq!(empty.status(), axum::http::StatusCode::BAD_REQUEST);
        let same = app.oneshot(post(title(node_id), json!({ "title": "Project X shipped its first release" }))).await.unwrap();
        assert_eq!(body_json(same).await["changed"], json!(false));

        let audit = agentops_teams::TeamStore::open(&teams_dir.path().join("teams.db")).unwrap().list_audit_log(&owner.tenant, None, None, None).unwrap();
        let retitles: Vec<_> = audit.iter().filter(|e| e.action == "node_retitled").collect();
        assert_eq!(retitles.len(), 1, "only the real change is logged: {audit:?}");
        assert!(retitles[0].metadata.contains("project-x") && retitles[0].metadata.contains("first release"), "{:?}", retitles[0]);
    }

    #[tokio::test]
    async fn hook_events_round_trip_through_the_connections_store_and_never_cross_tenants() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let (other, other_token) = signup(&accounts, "other@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&other.tenant, other.id, "admin").unwrap();
        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");
        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);
        let post = |event: &str, token: &str, payload: Value| {
            Request::builder()
                .method("POST")
                .uri(format!("/repos/{connection_id}/hooks/{event}"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap()
        };

        // A prompt is recorded in the connection's store...
        let resp = app.clone().oneshot(post("userpromptsubmit", &owner_token, json!({ "session_id": "s1", "prompt": "fix the login bug" }))).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        // ...and the resume guide built from that same store shows it.
        let resp = app.clone().oneshot(post("sessionstart", &owner_token, json!({ "session_id": "s1", "source": "resume" }))).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = body_json(resp).await;
        let context = body["output"]["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_default().to_string();
        assert!(context.contains("fix the login bug"), "{body:?}");

        let resp = app.clone().oneshot(post("nosuchevent", &owner_token, json!({}))).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
        let resp = app.oneshot(post("userpromptsubmit", &other_token, json!({ "session_id": "s1", "prompt": "x" }))).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND, "another tenant must never reach the owner's store");
    }

    #[tokio::test]
    async fn node_similarity_scores_stored_vectors_skips_missing_ones_and_never_crosses_tenants() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let (other, other_token) = signup(&accounts, "other@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&other.tenant, other.id, "admin").unwrap();
        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");

        let path = crate::indexing::checkout_path(checkouts_dir.path(), &owner.tenant, &connection_id);
        let (a, b, c, unembedded) = {
            let graph = agentops_mcp::open_store(&path).unwrap();
            let repo = agentops_mcp::repo_name(&path);
            let note = |name: &str| agentops_graph::upsert_node(graph.as_ref(), agentops_graph::NewNode { kind: agentops_graph::NodeKind::Note, repo: repo.clone(), path: Some(format!("vault:{name}")), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(name.into()) }).unwrap();
            let (a, b, c, d) = (note("a"), note("b"), note("c"), note("d"));
            let vector = |i: usize| {
                let mut v = vec![0.0_f32; agentops_embeddings::EMBEDDING_DIM];
                v[i] = 1.0;
                v
            };
            graph.set_embedding(&repo, a, &vector(0)).unwrap();
            graph.set_embedding(&repo, b, &vector(0)).unwrap();
            graph.set_embedding(&repo, c, &vector(1)).unwrap();
            (a, b, c, d)
        };
        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);
        let get = |uri: String, token: &str| Request::builder().uri(uri).header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap();

        let resp = app.clone().oneshot(get(format!("/repos/{connection_id}/nodes/{a}/similarity?to={b},{c},{unembedded},999999"), &owner_token)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let scores = body_json(resp).await["scores"].clone();
        assert!((scores[b.to_string()].as_f64().unwrap() - 1.0).abs() < 1e-6, "{scores}");
        assert!(scores[c.to_string()].as_f64().unwrap().abs() < 1e-6, "{scores}");
        assert_eq!(scores.as_object().unwrap().len(), 2, "a node with no vector, or no such node, is left out: {scores}");

        let resp = app.clone().oneshot(get(format!("/repos/{connection_id}/nodes/{a}/similarity"), &owner_token)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK, "an absent `to` must not 400");
        assert_eq!(body_json(resp).await["scores"], json!({}));
        let resp = app.clone().oneshot(get(format!("/repos/{connection_id}/nodes/{unembedded}/similarity?to={a}"), &owner_token)).await.unwrap();
        assert_eq!(body_json(resp).await["scores"], json!({}));

        let resp = app.oneshot(get(format!("/repos/{connection_id}/nodes/{a}/similarity?to={b}"), &other_token)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND, "another tenant must never reach the owner's store");
    }

    #[tokio::test]
    async fn recall_ranks_deeper_than_a_seed_records_no_session_and_never_crosses_tenants() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let (other, other_token) = signup(&accounts, "other@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&other.tenant, other.id, "admin").unwrap();
        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");

        let path = crate::indexing::checkout_path(checkouts_dir.path(), &owner.tenant, &connection_id);
        let repo = agentops_mcp::repo_name(&path);
        {
            let graph = agentops_mcp::open_store(&path).unwrap();
            for n in 0..60 {
                let name = format!("webhook signature note {n}");
                agentops_graph::upsert_node(graph.as_ref(), agentops_graph::NewNode { kind: agentops_graph::NodeKind::Gotcha, repo: repo.clone(), path: Some(format!("vault:{n}.md")), name: Some(name.clone()), container: None, start_line: None, end_line: None, content: Some(name) }).unwrap();
            }
        }
        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);
        let post = |token: &str, body: Value| {
            Request::builder()
                .method("POST")
                .uri(format!("/repos/{connection_id}/recall"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        let resp = app.clone().oneshot(post(&owner_token, json!({ "prompt": "webhook signature", "limit": 500 }))).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let pointers = body_json(resp).await["pointers"].as_array().unwrap().clone();
        // recall_seed's candidate pool (PER_QUERY per keyword and kind)
        // bounds the list as much as the limit does.
        assert!(pointers.len() > 8 && pointers.len() <= super::RECALL_DEEP_MAX, "deeper than a seed, capped at {}: {}", super::RECALL_DEEP_MAX, pointers.len());
        assert_eq!(pointers[0]["kind"], "gotcha");
        assert!(pointers[0]["node_id"].as_i64().is_some() && pointers[0]["reason"].as_str().is_some());
        let graph = agentops_mcp::open_store(&path).unwrap();
        assert!(graph.session_events_for_repo(&repo, None).unwrap().is_empty(), "a recall probe records nothing");

        let resp = app.oneshot(post(&other_token, json!({ "prompt": "webhook signature" }))).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND, "another tenant must never reach the owner's store");
    }

    #[tokio::test]
    async fn work_recall_resolves_a_file_cue_records_no_session_and_never_crosses_tenants() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let (other, other_token) = signup(&accounts, "other@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&other.tenant, other.id, "admin").unwrap();
        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");

        let path = crate::indexing::checkout_path(checkouts_dir.path(), &owner.tenant, &connection_id);
        let repo = agentops_mcp::repo_name(&path);
        let gotcha = {
            let graph = agentops_mcp::open_store(&path).unwrap();
            let node = |kind, path: &str, name: &str| agentops_graph::upsert_node(graph.as_ref(), agentops_graph::NewNode { kind, repo: repo.clone(), path: Some(path.into()), name: Some(name.into()), container: None, start_line: None, end_line: None, content: Some(name.into()) }).unwrap();
            let symbol = node(agentops_graph::NodeKind::Symbol, "src/hooks.rs", "handle");
            let gotcha = node(agentops_graph::NodeKind::Gotcha, "vault:g.md", "hook payload trap");
            graph.add_edge(&repo, gotcha, symbol, agentops_graph::EdgeRelation::Affects).unwrap();
            gotcha
        };
        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);
        let post = |token: &str, body: Value| {
            Request::builder()
                .method("POST")
                .uri(format!("/repos/{connection_id}/work-recall"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        let resp = app.clone().oneshot(post(&owner_token, json!({ "cue": { "kind": "file", "path": "src/hooks.rs" }, "sources": "affects_only" }))).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let candidates = body_json(resp).await["candidates"].as_array().unwrap().clone();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["node_id"], json!(gotcha));
        assert_eq!(candidates[0]["reason"], "affects `handle`");
        let graph = agentops_mcp::open_store(&path).unwrap();
        assert!(graph.session_events_for_repo(&repo, None).unwrap().is_empty(), "a work-recall probe records nothing");

        let resp = app.oneshot(post(&other_token, json!({ "cue": { "kind": "file", "path": "src/hooks.rs" } }))).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND, "another tenant must never reach the owner's store");
    }

    #[tokio::test]
    async fn usage_sync_then_usage_json_round_trips_and_never_crosses_tenants() {
        let (store, secrets) = test_state();
        let checkouts_dir = tempfile::tempdir().unwrap();
        let accounts = agentops_accounts::AccountStore::open_in_memory().unwrap();
        let (owner, owner_token) = signup(&accounts, "owner@example.com");
        let (other, other_token) = signup(&accounts, "other@example.com");
        let teams = agentops_teams::TeamStore::open_in_memory().unwrap();
        teams.add_member(&owner.tenant, owner.id, "admin").unwrap();
        teams.add_member(&other.tenant, other.id, "admin").unwrap();

        let connection_id = seed_active_indexed_connection(checkouts_dir.path(), &store, secrets.as_ref(), &owner.tenant, "widgets");

        let app = crate::build_router(store, secrets, None, None, None, std::path::PathBuf::from("unused-docbrain-dir"), Some(accounts), Some(teams), agentops_repo_access::indexing_store::IndexingStore::open_in_memory().unwrap(), checkouts_dir.path().to_path_buf(), None);

        let entries = json!({ "entries": [{
            "session_id": "sess-1",
            "model": "claude-sonnet-5",
            "input_tokens": 1000,
            "output_tokens": 500,
            "cache_read_tokens": 0,
            "cache_write_tokens": 0,
            "cost_estimate_usd": 1.5,
            "session_started_at": "2026-09-03T00:00:00Z",
            "session_ended_at": "2026-09-03T01:00:00Z",
        }] });
        let sync_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/repos/{connection_id}/usage/sync"))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(entries.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sync_resp.status(), axum::http::StatusCode::OK);
        assert_eq!(body_json(sync_resp).await["synced"], 1);

        let usage_resp = app
            .clone()
            .oneshot(Request::builder().uri(format!("/repos/{connection_id}/usage")).header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(usage_resp.status(), axum::http::StatusCode::OK);
        let usage_body = body_json(usage_resp).await;
        assert_eq!(usage_body["tokens"]["input_tokens"], 1000, "{usage_body:?}");
        assert_eq!(usage_body["tokens"]["output_tokens"], 500, "{usage_body:?}");

        // A newer CLI also sends per-session breakdowns; `repo` on the wire
        // is ignored in favour of the resolved connection.
        let with_reports = json!({ "entries": [], "reports": [{
            "repo": "someone-elses-repo",
            "session_id": "sess-1",
            "source": "transcript",
            "agent": "claude-code",
            "calls": 3,
            "total_context_tokens": 9000,
            "categories": { "tool:Bash": 6000, "baseline": 3000 },
            "resets": [{ "threshold": 150000, "resets": 0, "simulated_tokens": 9000, "savings": 0.0 }],
        }] });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/repos/{connection_id}/usage/sync"))
                    .header("authorization", format!("Bearer {owner_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(with_reports.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let usage_body = body_json(
            app.clone()
                .oneshot(Request::builder().uri(format!("/repos/{connection_id}/usage")).header("authorization", format!("Bearer {owner_token}")).body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(usage_body["reports"][0]["total_context_tokens"], 9000, "{usage_body:?}");
        assert_eq!(usage_body["reports"][0]["top_categories"][0][0], "tool:Bash", "{usage_body:?}");

        let other_resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/repos/{connection_id}/usage/sync"))
                    .header("authorization", format!("Bearer {other_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(entries.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(other_resp.status(), axum::http::StatusCode::NOT_FOUND, "a different tenant must never be able to push usage into another tenant's connection");
    }
}
