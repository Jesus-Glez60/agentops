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
use agentops_embeddings::{Embedder, LocalEmbedder};
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

/// `GET /local-search` -- tenant-scoped semantic/embedding search, reusing
/// `LocalEmbedder`/`GraphStore::search_similar` exactly the way
/// `agentops-api`'s `search_json` does.
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
        let embedding = LocalEmbedder.embed(&query_text)?;
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
                for (node, distance) in store.search_similar(name, &embedding, top_k, *kind)? {
                    hits.push(agentops_api::search::node_to_result(name, node, distance));
                }
            }
        }
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

#[derive(Debug, Deserialize)]
pub(crate) struct SetCurationBody {
    prominence: String,
    reason: Option<String>,
}

fn parse_prominence(s: &str) -> Option<NodeProminence> {
    match s {
        "full" => Some(NodeProminence::Full),
        "reduced" => Some(NodeProminence::Reduced),
        _ => None,
    }
}

/// `POST /repos/{id}/nodes/{node_id}/curation` -- path resolution mirrors
/// `node_detail_json` exactly.
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
    if prominence == NodeProminence::Reduced && reason.is_none() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "'reason' is required (non-empty) when prominence is 'reduced'" })));
    }
    let reason = reason.map(str::to_string);

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
    session_started_at: String,
    session_ended_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UsageSyncBody {
    entries: Vec<UsageSyncEntry>,
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
                session_started_at: entry.session_started_at.clone(),
                session_ended_at: entry.session_ended_at.clone(),
            })?;
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
            for node in store.nodes_by_kind(name, NodeKind::Gotcha)? {
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
