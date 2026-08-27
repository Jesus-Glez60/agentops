//! Driving (inbound) adapter: light-tier REST endpoints, wrapping the same
//! `agentops-mcp` tool dispatch the stdio MCP server uses — one
//! implementation of each tool, two transports, mirroring `docbrain-api`'s
//! identical relationship to `docbrain-mcp`.
//!
//! Unlike `docbrain-api`, there's no shared, pre-opened store here: every
//! `agentops-mcp` tool takes a repo `path` in its own arguments and opens
//! its own `GraphStore` per call via `agentops_mcp::open_store` (a design
//! that supports one server process serving requests about many different
//! repos, unlike docbrain's one-store-per-process model) — so `AppState`
//! only needs the fixed `AccessMode` and optional API-key hash for this
//! process.

use std::path::PathBuf;

use agentops_graph::NodeKind;
use agentops_mcp::AccessMode;
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

// `pub` -- `agentops-heavy-api` reuses the free functions in these modules
// (tenant-scoped dashboard routes composing the same logic against a
// `ConnectionStore`-resolved checkout path instead of a manifest-derived
// one) rather than duplicating them. See each module's specific `pub fn`s
// for what's actually meant to be called cross-crate; everything else stays
// `pub(crate)`/private.
pub mod docs;
pub mod repos;
pub mod search;
pub mod subgraph;

#[derive(Clone)]
pub(crate) struct AppState {
    mode: AccessMode,
    /// SHA-256 hash of the required API key. `None` disables auth.
    api_key_hash: Option<String>,
    /// Where `agentops-manifest`'s local scan registry lives for this
    /// process — explicit rather than each handler resolving
    /// `$HOME/.agentops/manifest.json` (or an env var) itself, both so
    /// `run()` is the one place that decision is made and so tests can
    /// inject an isolated tempdir path instead of racing on real global
    /// state under parallel test execution.
    pub(crate) manifest_path: PathBuf,
}

/// `api_key_hash`, if set, requires every request except `/health` to
/// present a matching `Authorization: Bearer <key>` header. Local scan
/// registry listing lives at `/scans` and code search at `/local-search` —
/// deliberately not `/repos`/`/search` (the paths a merged multi-service
/// process gives to the hosted-connections/semantic-search endpoints of the
/// same name), so a process composing this router alongside others has no
/// path collision to resolve.
pub fn build_router(mode: AccessMode, api_key_hash: Option<String>, manifest_path: PathBuf) -> Router {
    build_router_without_health(mode, api_key_hash, manifest_path).merge(health_router())
}

/// Same as [`build_router`] minus the `/health` route — for composing this
/// service's routes into a larger process (e.g. the merged `agentops-server`
/// binary) that mounts its own single shared `/health` instead of one copy
/// per merged service (`Router::merge` panics on a duplicate route).
///
/// Includes the dashboard routes (`/activity`, `/local-search`, `/gotchas`,
/// `/repos/{name}/nodes/{id}*`, `/repos/{name}/graph`, `/repos/{name}/docs`)
/// — see [`build_router_without_dashboard_routes`] for the merged-server
/// variant that excludes them.
pub fn build_router_without_health(mode: AccessMode, api_key_hash: Option<String>, manifest_path: PathBuf) -> Router {
    build_router_with_dashboard_flag(mode, api_key_hash, manifest_path, true)
}

/// Same as [`build_router_without_health`] minus the eight single-operator
/// "dashboard" routes (`/activity`, `/local-search`, `/gotchas`,
/// `/repos/{name}/nodes/{id}`, `/repos/{name}/nodes/{id}/curation`,
/// `/repos/{name}/nodes/{id}/graph`, `/repos/{name}/graph`,
/// `/repos/{name}/docs`) — these read from `agentops-manifest`'s local
/// `~/.agentops/manifest.json`, which a tenant-scoped hosted deployment
/// never populates. `agentops-heavy-api` registers tenant-scoped routes at
/// these exact same paths (resolved via `ConnectionStore` instead of the
/// manifest) for its own dashboard screens; `Router::merge` panics on a
/// duplicate route, so a process composing both crates (e.g.
/// `agentops-server`) must mount only one implementation per path — this is
/// that exclusion, mirroring `agentops-heavy-api`'s own
/// `build_router`/`build_router_without_tools` split for the identical
/// reason.
pub fn build_router_without_dashboard_routes(mode: AccessMode, api_key_hash: Option<String>, manifest_path: PathBuf) -> Router {
    build_router_with_dashboard_flag(mode, api_key_hash, manifest_path, false)
}

fn build_router_with_dashboard_flag(mode: AccessMode, api_key_hash: Option<String>, manifest_path: PathBuf, include_dashboard_routes: bool) -> Router {
    let state = AppState { mode, api_key_hash, manifest_path };
    let mut router = Router::new()
        .route("/tools", get(list_tools_handler))
        .route("/tools/{name}", post(call_tool_handler))
        .route("/nodes", get(list_nodes_json))
        .route("/scans", get(repos::list_repos_json))
        .route("/repos/{name}/rescan", post(repos::rescan_repo_json))
        .route("/repos/{name}/edges/{id}/reinforce", post(subgraph::reinforce_edge_json));
    if include_dashboard_routes {
        router = router
            .route("/repos/{name}/nodes/{id}", get(search::node_detail_json))
            .route("/repos/{name}/nodes/{id}/curation", post(search::set_curation_json))
            .route("/repos/{name}/nodes/{id}/graph", get(subgraph::subgraph_json))
            .route("/repos/{name}/graph", get(subgraph::repo_graph_json))
            .route("/repos/{name}/docs", get(docs::docs_json))
            .route("/activity", get(repos::activity_json))
            .route("/local-search", get(search::search_json))
            .route("/gotchas", get(repos::gotchas_json));
    }
    router
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

fn health_router() -> Router {
    Router::new().route("/health", get(health)).layer(CorsLayer::permissive())
}

async fn require_api_key(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match agentops_security::api_key::check_bearer_api_key(req.headers(), state.api_key_hash.as_deref()) {
        Ok(()) => next.run(req).await,
        Err((status, body)) => (status, body).into_response(),
    }
}

/// Binds `addr` and serves until the process is killed. `mode` is fixed for
/// the process's lifetime. If `AGENTOPS_API_KEY_HASH` is set in the
/// environment, every request except `/health` requires a matching
/// `Authorization: Bearer <key>` header.
pub async fn run(addr: &str, mode: AccessMode) -> anyhow::Result<()> {
    let api_key_hash = std::env::var("AGENTOPS_API_KEY_HASH").ok();
    let auth_status = if api_key_hash.is_some() { "API key required" } else { "UNAUTHENTICATED (set AGENTOPS_API_KEY_HASH to require a key)" };
    let manifest_path = std::env::var("AGENTOPS_MANIFEST_PATH").map(PathBuf::from).unwrap_or_else(|_| agentops_manifest::default_manifest_path());
    let app = build_router(mode, api_key_hash, manifest_path);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("agentops-api listening on {addr} (mode: {mode:?}, auth: {auth_status})");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn list_tools_handler(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "tools": agentops_mcp::list_tools(state.mode) }))
}

/// `spawn_blocking` -- required, not just a performance nicety.
/// `agentops-graph-pg`'s own doc comment documents that `GraphStore` calls
/// (which most `agentops_mcp` tools make via `open_store`) `block_on` an
/// internally-owned Tokio runtime when `AGENTOPS_DATABASE_URL` is set, and
/// that an async caller "must never call from inside an already-running
/// Tokio runtime directly (nested `block_on` panics)". This handler is
/// exactly that async caller. Confirmed live: calling this route
/// unwrapped against a real Postgres-backed deployment panics the worker
/// thread with "Cannot start a runtime from within a runtime" and drops
/// the connection with an empty reply, no error body at all -- caught
/// testing the new `/mcp` route (`agentops-heavy-api::mcp_http`) against
/// `thedamnserver`, which surfaced that this pre-existing route had the
/// identical bug, just never exercised against Postgres before.
async fn call_tool_handler(State(state): State<AppState>, AxumPath(name): AxumPath<String>, body: Option<Json<Value>>) -> (StatusCode, Json<Value>) {
    let empty = json!({});
    let args = body.map(|Json(v)| v).unwrap_or(empty);

    let result = tokio::task::spawn_blocking(move || agentops_mcp::call_tool(state.mode, &name, &args)).await;
    match result {
        Ok(Ok(result)) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())),
        Ok(Err(refusal)) => (StatusCode::NOT_FOUND, Json(json!({ "error": refusal }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("tool call panicked: {e}") }))),
    }
}

#[derive(Debug, Deserialize)]
struct NodesQuery {
    path: String,
    kind: Option<String>,
}

/// `pub` -- `search.rs`/`subgraph.rs` reuse this instead of a second copy of
/// the same string-to-`NodeKind` mapping, and so does `agentops-heavy-api`'s
/// own kind-filter parsing for its tenant-scoped dashboard routes.
pub fn parse_kind(s: &str) -> Option<NodeKind> {
    match s.to_lowercase().as_str() {
        "symbol" => Some(NodeKind::Symbol),
        "file" => Some(NodeKind::File),
        "gotcha" => Some(NodeKind::Gotcha),
        "decision" => Some(NodeKind::Decision),
        "definition" => Some(NodeKind::Definition),
        "note" => Some(NodeKind::Note),
        _ => None,
    }
}

/// Structured JSON for a dashboard's node browser — `/tools/*` returns
/// MCP-shaped text content (right for an agent, awkward for a UI to render
/// as a table), so this is a small, genuinely-REST endpoint alongside them,
/// same reasoning as `docbrain-api`'s `/libraries`.
///
/// `agentops_mcp::open_store` blocks its calling thread during I/O when
/// `AGENTOPS_DATABASE_URL` selects `PostgresGraphStore` (see that store's
/// doc comment on the sync bridge) — run inside `spawn_blocking` so a slow
/// Postgres round-trip can't stall this process's async executor. Cheap
/// for the SQLite default too, just an unneeded (harmless) thread hop.
async fn list_nodes_json(Query(q): Query<NodesQuery>) -> (StatusCode, Json<Value>) {
    let kind = match q.kind.as_deref().map(parse_kind) {
        Some(None) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid 'kind'" }))),
        Some(Some(kind)) => Some(kind),
        None => None,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<agentops_graph::Node>> {
        let repo_path = PathBuf::from(&q.path);
        let store = agentops_mcp::open_store(&repo_path)?;
        let repo = repo_path.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or(q.path.clone());
        match kind {
            Some(kind) => store.nodes_by_kind(&repo, kind),
            None => store.all_nodes(&repo),
        }
    })
    .await;

    match result {
        Ok(Ok(nodes)) => (StatusCode::OK, Json(json!({ "nodes": nodes }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// A manifest path pointing nowhere real yet -- `list_scanned_repos_at`
    /// handles `NotFound` as an empty list, so tests that don't care about
    /// `/repos` data can use this rather than touching the real
    /// `$HOME/.agentops/manifest.json` (which would race other tests/the
    /// developer's own real manifest under parallel test execution).
    fn empty_manifest_path() -> PathBuf {
        tempfile::tempdir().unwrap().keep().join("manifest.json")
    }

    #[tokio::test]
    async fn health_check_ok() {
        let app = build_router(AccessMode::Advisor, None, empty_manifest_path());
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tools_list_respects_access_mode_over_http() {
        let advisor_app = build_router(AccessMode::Advisor, None, empty_manifest_path());
        let full_app = build_router(AccessMode::Full, None, empty_manifest_path());

        let advisor_resp = advisor_app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        let advisor_body = body_json(advisor_resp).await;
        let full_resp = full_app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        let full_body = body_json(full_resp).await;

        assert!(full_body["tools"].as_array().unwrap().len() > advisor_body["tools"].as_array().unwrap().len());
    }

    #[tokio::test]
    async fn calling_a_tool_over_http_reuses_the_same_dispatch_as_mcp() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        let path = dir.path().to_string_lossy().to_string();

        let app = build_router(AccessMode::Full, None, empty_manifest_path());
        let req = Request::builder()
            .method("POST")
            .uri("/tools/scan_repo")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "path": path }).to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["isError"], false);

        let nodes_req = Request::builder().uri(format!("/nodes?path={path}&kind=file")).body(Body::empty()).unwrap();
        let nodes_resp = app.oneshot(nodes_req).await.unwrap();
        assert_eq!(nodes_resp.status(), StatusCode::OK);
        let nodes_body = body_json(nodes_resp).await;
        assert_eq!(nodes_body["nodes"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_full_only_tool_is_rejected_over_http_under_advisor_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let app = build_router(AccessMode::Advisor, None, empty_manifest_path());
        let req = Request::builder()
            .method("POST")
            .uri("/tools/scan_repo")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "path": path }).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "a refused tool is a tool-level error, not a transport error");
        let body = body_json(resp).await;
        assert_eq!(body["isError"], true);
    }

    #[tokio::test]
    async fn health_check_bypasses_auth_even_when_a_key_is_required() {
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(AccessMode::Advisor, Some(hash), empty_manifest_path());
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_api_key_is_rejected_when_one_is_required() {
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(AccessMode::Advisor, Some(hash), empty_manifest_path());
        let resp = app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn correct_api_key_is_accepted() {
        let (raw, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(AccessMode::Advisor, Some(hash), empty_manifest_path());
        let req = Request::builder().uri("/tools").header("authorization", format!("Bearer {raw}")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
