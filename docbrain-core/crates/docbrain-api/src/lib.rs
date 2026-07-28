//! REST API server (axum) wrapping `docbrain-mcp`'s tool logic directly — one
//! implementation of each tool (list_libraries, get_docs, discover_library,
//! ...), two transports, same as the `agentops-api`/`agentops-mcp` pattern.
//!
//! `DocbrainStore` isn't `Sync` (SQLite connections aren't shared across
//! threads), so it's wrapped in `Arc<Mutex<_>>` for axum's multi-threaded
//! handler pool — every handler locks it for the duration of one call.

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use docbrain_graph::{DocbrainStore, TenantContext};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct AppState {
    store: Arc<Mutex<DocbrainStore>>,
}

pub fn build_router(store: DocbrainStore) -> Router {
    let state = AppState { store: Arc::new(Mutex::new(store)) };
    Router::new()
        .route("/health", get(health))
        .route("/tools", get(list_tools_handler))
        .route("/tools/{name}", post(call_tool_handler))
        .route("/libraries", get(list_libraries_json))
        .with_state(state)
        // Permissive CORS: this server has no auth of its own yet (see
        // SECURITY.md), and the dashboard is a separate origin (Next.js dev
        // server) during local development — fine for localhost-only Phase 2
        // use, revisit before this is ever bound beyond 127.0.0.1.
        .layer(CorsLayer::permissive())
}

pub async fn run(addr: &str, db_path: &Path) -> anyhow::Result<()> {
    let store = DocbrainStore::open(db_path)?;
    let app = build_router(store);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("docbrain-api listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn list_tools_handler() -> Json<Value> {
    Json(json!({ "tools": docbrain_mcp::list_tools() }))
}

#[derive(Debug, Deserialize)]
struct LibrariesQuery {
    org: Option<String>,
}

/// Structured JSON for the dashboard's library browser — the `tools/*`
/// endpoints return MCP-shaped text content (right for an agent, awkward for
/// a UI to render as a table), so this is a small, genuinely-REST endpoint
/// alongside them rather than asking the frontend to parse tool-result text.
async fn list_libraries_json(State(state): State<AppState>, Query(q): Query<LibrariesQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match q.org {
        Some(org) => TenantContext::org(org),
        None => TenantContext::public(),
    };

    let store = state.store.lock().unwrap();
    match store.list_libraries(&tenant) {
        Ok(libs) => (StatusCode::OK, Json(json!({ "libraries": libs }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

async fn call_tool_handler(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    body: Option<Json<Value>>,
) -> (StatusCode, Json<Value>) {
    let empty = json!({});
    let args = body.map(|Json(v)| v).unwrap_or(empty);

    let store = state.store.lock().unwrap();
    match docbrain_mcp::call_tool(&store, &name, &args) {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())),
        Err(refusal) => (StatusCode::FORBIDDEN, Json(json!({ "error": refusal }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use docbrain_graph::{TenantContext, Visibility};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_check_ok() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let app = build_router(store);
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn libraries_json_endpoint_returns_structured_data_scoped_by_org() {
        let store = DocbrainStore::open_in_memory().unwrap();
        store.add_library(&TenantContext::public(), "react", "React", None, Some("https://react.dev"), Visibility::Public).unwrap();
        store
            .add_library(&TenantContext::org("acme"), "acme-sdk", "Acme SDK", None, None, Visibility::Private("acme".into()))
            .unwrap();

        let app = build_router(store);
        let resp = app.clone().oneshot(Request::builder().uri("/libraries").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let libs = body["libraries"].as_array().unwrap();
        assert_eq!(libs.len(), 1, "public caller should only see the public library");
        assert_eq!(libs[0]["slug"], "react");

        let resp = app.oneshot(Request::builder().uri("/libraries?org=acme").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        assert_eq!(body["libraries"].as_array().unwrap().len(), 2, "acme should see public + its own private library");
    }

    #[tokio::test]
    async fn tools_list_has_five_entries() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let app = build_router(store);
        let resp = app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        let body = body_json(resp).await;
        assert_eq!(body["tools"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn private_library_is_not_leaked_across_orgs_over_http() {
        let store = DocbrainStore::open_in_memory().unwrap();
        store
            .add_library(&TenantContext::org("acme"), "acme-sdk", "Acme SDK", None, None, Visibility::Private("acme".into()))
            .unwrap();

        let app = build_router(store);
        let req = Request::builder()
            .method("POST")
            .uri("/tools/get_library")
            .header("content-type", "application/json")
            .body(Body::from(json!({"slug": "acme-sdk", "org": "globex"}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK); // tool-level error, not a transport error
        let body = body_json(resp).await;
        assert_eq!(body["isError"], true);
    }
}
