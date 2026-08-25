//! Driving (inbound) adapter: REST endpoints for docbrain, wrapping the
//! same `docbrain-mcp` tool dispatch that the stdio MCP server uses — one
//! implementation of each tool (list_libraries, search_docs, ...), two
//! transports. `docbrain-mcp`'s `call_tool`/`list_tools` are themselves
//! thin (they delegate to `docbrain_ingest::retrieve` etc.), so depending
//! on them here isn't duplicating business logic across adapters — it's
//! two driving adapters sharing one dispatch/formatting layer, same as the
//! `agentops-api`/`agentops-mcp` pattern this mirrors.
//!
//! `SqliteDocbrainStore` isn't `Sync` (SQLite connections aren't shared
//! across threads), so it's wrapped in `Arc<Mutex<_>>` for axum's
//! multi-threaded handler pool — every handler locks it for the duration
//! of one call.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use docbrain_graph::{DocbrainStore, SqliteDocbrainStore};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct AppState {
    store: Arc<Mutex<SqliteDocbrainStore>>,
    /// Needed only by tools that can run in the background
    /// (`scrape_library`/`sync_changelogs` with `background: true`) — the
    /// spawned thread opens its own store connection from this path rather
    /// than sharing `store` across threads. See `docbrain_mcp::call_tool`'s
    /// doc comment.
    db_path: PathBuf,
    /// SHA-256 hash of the required API key. `None` disables auth.
    api_key_hash: Option<String>,
}

/// `api_key_hash`, if set, requires every request except `/health` to
/// present a matching `Authorization: Bearer <key>` header.
pub fn build_router(store: SqliteDocbrainStore, db_path: PathBuf, api_key_hash: Option<String>) -> Router {
    build_router_without_health(store, db_path, api_key_hash).merge(health_router())
}

/// Same as [`build_router`] minus the `/health` route — for composing this
/// service's routes into a larger process (e.g. the merged `agentops-server`
/// binary) that mounts its own single shared `/health` instead of one copy
/// per merged service (`Router::merge` panics on a duplicate route).
pub fn build_router_without_health(store: SqliteDocbrainStore, db_path: PathBuf, api_key_hash: Option<String>) -> Router {
    let state = AppState { store: Arc::new(Mutex::new(store)), db_path, api_key_hash };
    Router::new()
        .route("/tools", get(list_tools_handler))
        .route("/tools/{name}", post(call_tool_handler))
        .route("/libraries", get(list_libraries_json))
        .route("/libraries/{slug}", get(get_library_json))
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .with_state(state)
        // Permissive CORS: identity/authorization rides on the
        // Authorization header (checked above), not on request origin.
        .layer(CorsLayer::permissive())
}

/// Same as [`build_router_without_health`] minus `/tools`/`/tools/{name}` —
/// `agentops-api` registers the same two paths for its own tool table, so a
/// process composing both (e.g. `agentops-server`) can only mount one of
/// them as-is; the other needs a merged dispatcher covering both tool
/// tables, not yet built (tracked for the MCP-server-merge follow-up). Until
/// then, the merged process keeps `agentops-api`'s `/tools` and mounts this
/// variant instead of [`build_router_without_health`]; `/libraries` is
/// unaffected either way.
pub fn build_router_without_health_and_tools(store: SqliteDocbrainStore, db_path: PathBuf, api_key_hash: Option<String>) -> Router {
    let state = AppState { store: Arc::new(Mutex::new(store)), db_path, api_key_hash };
    Router::new()
        .route("/libraries", get(list_libraries_json))
        .route("/libraries/{slug}", get(get_library_json))
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

/// Binds `addr` and serves until the process is killed. If
/// `AGENTOPS_API_KEY_HASH` is set in the environment, every request except
/// `/health` requires a matching `Authorization: Bearer <key>` header.
pub async fn run(addr: &str, db_path: &Path) -> anyhow::Result<()> {
    let store = SqliteDocbrainStore::open(db_path)?;
    let api_key_hash = std::env::var("AGENTOPS_API_KEY_HASH").ok();
    let auth_status = if api_key_hash.is_some() { "API key required" } else { "UNAUTHENTICATED (set AGENTOPS_API_KEY_HASH to require a key)" };
    let app = build_router(store, db_path.to_path_buf(), api_key_hash);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("docbrain-api listening on {addr} (auth: {auth_status})");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn list_tools_handler() -> Json<Value> {
    Json(json!({ "tools": docbrain_mcp::list_tools() }))
}

/// Structured JSON for a dashboard's library browser — the `/tools/*`
/// endpoints return MCP-shaped text content (right for an agent, awkward
/// for a UI to render as a table), so this is a small, genuinely-REST
/// endpoint alongside them rather than asking a frontend to parse
/// tool-result text.
async fn list_libraries_json(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let store = state.store.lock().unwrap();
    let libs = match store.list_libraries() {
        Ok(libs) => libs,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    // Only libraries with real ingested content -- `sync_docs`'s
    // import/manifest-based auto-discovery registers a name+registry-metadata
    // row for every dependency it finds, whether or not anyone ever scrapes
    // its docs. Surfacing all of those here would make this a mirror of a
    // repo's package.json, not "what docbrain actually has" -- the dashboard's
    // one real consumer of this endpoint. `list_libraries` (the MCP tool) is
    // unaffected: agents still see the full registry there.
    let libs: Vec<_> = libs.into_iter().filter(|lib| !lib.versions.is_empty()).collect();

    // `has_mismatch` per row, same derived-at-read-time comparison as the
    // detail endpoint — N+1 `repos_using_library` calls, one per library,
    // acceptable at the table sizes a self-hosted docbrain instance
    // actually has (same tradeoff already accepted in `stats_for`).
    let libs_json: Vec<Value> = libs
        .into_iter()
        .map(|lib| {
            let has_mismatch = store
                .repos_using_library(&lib.slug)
                .map(|used_in| used_in.iter().any(|u| lib.versions.last().is_some_and(|latest| latest != &u.declared_version)))
                .unwrap_or(false);
            let mut value = serde_json::to_value(&lib).unwrap();
            value["has_mismatch"] = json!(has_mismatch);
            value
        })
        .collect();

    (StatusCode::OK, Json(json!({ "libraries": libs_json })))
}

/// Detail-by-slug JSON — the mockup's identity block, tabs, and
/// repos-using-this sidebar all need one library plus its `used_in` list
/// plus a derived mismatch flag, which `/tools/get_library`'s MCP-text
/// format can't give a UI cheaply.
async fn get_library_json(State(state): State<AppState>, AxumPath(slug): AxumPath<String>) -> (StatusCode, Json<Value>) {
    let store = state.store.lock().unwrap();
    let library = match store.get_library(&slug) {
        Ok(Some(library)) => library,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("no library '{slug}' registered") }))),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    let used_in = match store.repos_using_library(&slug) {
        Ok(used_in) => used_in,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    // Mismatch is derived at read time against the latest indexed version
    // (versions come back ascending from the store), never stored.
    let latest_version = library.versions.last().cloned();
    let used_in_json: Vec<Value> = used_in
        .iter()
        .map(|u| {
            json!({
                "repo_identifier": u.repo_identifier,
                "declared_version": u.declared_version,
                "updated_at": u.updated_at,
                "mismatch": latest_version.as_deref().is_some_and(|latest| latest != u.declared_version),
            })
        })
        .collect();

    (StatusCode::OK, Json(json!({ "library": library, "used_in": used_in_json })))
}

async fn call_tool_handler(State(state): State<AppState>, AxumPath(name): AxumPath<String>, body: Option<Json<Value>>) -> (StatusCode, Json<Value>) {
    let empty = json!({});
    let args = body.map(|Json(v)| v).unwrap_or(empty);

    let store = state.store.lock().unwrap();
    match docbrain_mcp::call_tool(&*store, &state.db_path, &name, &args) {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())),
        Err(refusal) => (StatusCode::NOT_FOUND, Json(json!({ "error": refusal }))),
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

    #[tokio::test]
    async fn health_check_ok() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let app = build_router(store, PathBuf::from("unused.db"), None);
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn libraries_json_endpoint_returns_structured_data() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("react", "React", None, None, Some("https://react.dev")).unwrap();
        store.add_doc_snapshot("react", "19.1.0").unwrap();

        let app = build_router(store, PathBuf::from("unused.db"), None);
        let resp = app.oneshot(Request::builder().uri("/libraries").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let libs = body["libraries"].as_array().unwrap();
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0]["slug"], "react");
    }

    #[tokio::test]
    async fn libraries_json_endpoint_omits_registered_but_never_scraped_libraries() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        // Auto-discovered via sync_docs's import/manifest scanning -- name +
        // registry metadata only, never actually scraped.
        store.add_library("left-pad", "left-pad", None, None, Some("https://npmjs.com/left-pad")).unwrap();

        let app = build_router(store, PathBuf::from("unused.db"), None);
        let resp = app.oneshot(Request::builder().uri("/libraries").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        assert_eq!(body["libraries"].as_array().unwrap().len(), 0, "a library with no doc_snapshot must not appear in the dashboard's list");
    }

    #[tokio::test]
    async fn get_library_json_returns_404_for_an_unknown_slug() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let app = build_router(store, PathBuf::from("unused.db"), None);
        let resp = app.oneshot(Request::builder().uri("/libraries/nope").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_library_json_includes_used_in_with_a_derived_mismatch_flag() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("next", "Next.js", None, None, Some("https://nextjs.org")).unwrap();
        store.add_doc_snapshot("next", "16.2.12").unwrap();
        store.upsert_repo_library_version("/repos/app", "next", "15.1.0").unwrap();
        store.upsert_repo_library_version("/repos/other", "next", "16.2.12").unwrap();

        let app = build_router(store, PathBuf::from("unused.db"), None);
        let resp = app.oneshot(Request::builder().uri("/libraries/next").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;

        assert_eq!(body["library"]["slug"], "next");
        let used_in = body["used_in"].as_array().unwrap();
        assert_eq!(used_in.len(), 2);
        let app_row = used_in.iter().find(|u| u["repo_identifier"] == "/repos/app").unwrap();
        assert_eq!(app_row["mismatch"], true, "declared 15.1.0 vs latest indexed 16.2.12 must be flagged");
        let other_row = used_in.iter().find(|u| u["repo_identifier"] == "/repos/other").unwrap();
        assert_eq!(other_row["mismatch"], false, "declared version matching the latest indexed version is not a mismatch");
    }

    #[tokio::test]
    async fn tools_list_matches_the_mcp_tool_table() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let app = build_router(store, PathBuf::from("unused.db"), None);
        let resp = app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        assert_eq!(body["tools"].as_array().unwrap().len(), docbrain_mcp::list_tools().len());
    }

    #[tokio::test]
    async fn calling_a_tool_over_http_reuses_the_same_dispatch_as_mcp() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("react", "React", None, None, Some("https://react.dev")).unwrap();

        let app = build_router(store, PathBuf::from("unused.db"), None);
        let req = Request::builder()
            .method("POST")
            .uri("/tools/get_library")
            .header("content-type", "application/json")
            .body(Body::from(json!({"slug": "react"}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["isError"], false);
        assert!(body["content"][0]["text"].as_str().unwrap().contains("react"));
    }

    #[tokio::test]
    async fn health_check_bypasses_auth_even_when_a_key_is_required() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, PathBuf::from("unused.db"), Some(hash));
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_api_key_is_rejected_when_one_is_required() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, PathBuf::from("unused.db"), Some(hash));
        let resp = app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn correct_api_key_is_accepted() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let (raw, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(store, PathBuf::from("unused.db"), Some(hash));
        let req = Request::builder().uri("/tools").header("authorization", format!("Bearer {raw}")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
