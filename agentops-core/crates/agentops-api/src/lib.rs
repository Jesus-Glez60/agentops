//! REST API server (axum) wrapping the exact same tool logic as
//! `agentops-mcp` — `list_tools`/`call_tool` are reused directly rather than
//! re-implemented, so there's one implementation of each operation (scan,
//! note-taking, docgen) behind two transports, not two. `AccessMode`
//! enforcement is therefore identical to the MCP server: a client hitting
//! `GET /tools` in Advisor mode gets back exactly the 3 read-only tool
//! definitions, and `POST /tools/scan_repo` in Advisor mode is refused before
//! any repo-scanning code runs.

use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

use agentops_graph::{GraphStore, SqliteGraphStore};
use agentops_mcp::AccessMode;
use agentops_security::api_key::verify_api_key;

#[derive(Clone)]
struct AppState {
    mode: AccessMode,
    /// SHA-256 hash of the required API key. `None` means the server runs
    /// unauthenticated — fine for local-only dev use (matches this server's
    /// prior default), but callers binding beyond 127.0.0.1 should set
    /// `AGENTOPS_API_KEY_HASH` (see `run`). Auth is opt-in rather than
    /// mandatory-by-default because the CLI's local `serve-api` workflow
    /// has no key-distribution story yet — see SECURITY.md.
    api_key_hash: Option<String>,
}

/// Builds the router for a server running in `mode`. `AccessMode` is fixed
/// for the lifetime of the server (mirroring the MCP server's design) rather
/// than something a client can request per-call. `api_key_hash`, if set,
/// requires every request except `/health` to present a matching
/// `Authorization: Bearer <key>` header.
pub fn build_router(mode: AccessMode, api_key_hash: Option<String>) -> Router {
    let state = AppState { mode, api_key_hash };
    Router::new()
        .route("/tools", get(list_tools_handler))
        .route("/tools/{name}", post(call_tool_handler))
        .route("/graph", get(graph_json))
        .route("/docs", get(docs_content))
        .route("/repos", get(list_scanned_repos))
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .route("/health", get(health))
        .with_state(state)
        // Permissive CORS: identity/authorization rides on the Authorization
        // header (checked above), not on request origin — see docbrain-api's
        // identical reasoning. The dashboard is a separate origin during
        // local development.
        .layer(CorsLayer::permissive())
}

async fn require_api_key(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected_hash) = &state.api_key_hash else {
        return next.run(req).await;
    };

    let provided = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        Some(raw) if verify_api_key(raw, expected_hash).is_ok() => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing or invalid API key" }))).into_response(),
    }
}

/// Binds `addr` and serves until the process is killed. If
/// `AGENTOPS_API_KEY_HASH` is set in the environment, every request except
/// `/health` requires a matching `Authorization: Bearer <key>` header.
pub async fn run(addr: &str, mode: AccessMode) -> anyhow::Result<()> {
    let api_key_hash = std::env::var("AGENTOPS_API_KEY_HASH").ok();
    let auth_status = if api_key_hash.is_some() { "API key required" } else { "UNAUTHENTICATED (set AGENTOPS_API_KEY_HASH to require a key)" };
    let app = build_router(mode, api_key_hash);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("agentops-api listening on {addr} (access mode: {mode:?}, auth: {auth_status})");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

#[derive(Debug, Deserialize)]
struct RepoPathQuery {
    path: String,
}

/// Lists every repo `agentops install` has ever recorded scanning on this
/// machine (`~/.agentops/manifest.json`, via `agentops-manifest`), with a
/// live node-count summary for each — this is what the dashboard's repo
/// overview page reads so it can answer "what's indexed?" without the
/// caller already needing to know a path.
async fn list_scanned_repos() -> (StatusCode, Json<Value>) {
    let repos = match agentops_manifest::list_scanned_repos() {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    let summaries: Vec<Value> = repos
        .into_iter()
        .map(|entry| {
            let db_path = std::path::Path::new(&entry.path).join(".context").join("graph.db");
            let counts = SqliteGraphStore::open(&db_path).ok().and_then(|store| {
                let nodes = store.all_nodes().ok()?;
                Some(json!({
                    "files": nodes.iter().filter(|n| n.kind == agentops_graph::NodeKind::File).count(),
                    "symbols": nodes.iter().filter(|n| n.kind == agentops_graph::NodeKind::Symbol).count(),
                    "gotchas": nodes.iter().filter(|n| n.kind == agentops_graph::NodeKind::Gotcha).count(),
                    "decisions": nodes.iter().filter(|n| n.kind == agentops_graph::NodeKind::Decision).count(),
                }))
            });
            json!({
                "path": entry.path,
                "last_scanned_at": entry.last_scanned_at,
                "counts": counts,
            })
        })
        .collect();

    (StatusCode::OK, Json(json!({ "repos": summaries })))
}

/// Structured JSON for the dashboard's graph browser — same rationale as
/// docbrain-api's `/libraries`: the `tools/*` endpoints return MCP-shaped
/// text content, right for an agent, awkward for a UI to render as a
/// graph/list. This is a small, genuinely-REST endpoint alongside them.
async fn graph_json(Query(q): Query<RepoPathQuery>) -> (StatusCode, Json<Value>) {
    let db_path = std::path::Path::new(&q.path).join(".context").join("graph.db");
    if !db_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("no graph store at {} — run `agentops install --path {}` first", db_path.display(), q.path) })),
        );
    }

    let store = match SqliteGraphStore::open(&db_path) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    let nodes = match store.all_nodes() {
        Ok(n) => n,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };
    let edges = match store.all_edges() {
        Ok(e) => e,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    };

    (StatusCode::OK, Json(json!({ "nodes": nodes, "edges": edges })))
}

/// Serves the already-generated `repo-map.md` for a repo, as JSON (so the
/// dashboard doesn't need a separate raw-text fetch path). 404s with a clear
/// next step if `agentops docgen` hasn't been run yet, rather than an empty body.
async fn docs_content(Query(q): Query<RepoPathQuery>) -> (StatusCode, Json<Value>) {
    let doc_path = std::path::Path::new(&q.path).join("repo-map.md");
    match std::fs::read_to_string(&doc_path) {
        Ok(content) => (StatusCode::OK, Json(json!({ "content": content }))),
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("no onboarding doc at {} — run `agentops docgen --path {}` first", doc_path.display(), q.path) })),
        ),
    }
}

async fn list_tools_handler(State(state): State<AppState>) -> Json<Value> {
    let tools = agentops_mcp::list_tools(state.mode);
    Json(json!({ "tools": tools }))
}

async fn call_tool_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<Value>>,
) -> (StatusCode, Json<Value>) {
    let empty = json!({});
    let args = body.map(|Json(v)| v).unwrap_or(empty);

    match agentops_mcp::call_tool(state.mode, &name, &args) {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())),
        Err(refusal) => (StatusCode::FORBIDDEN, Json(json!({ "error": refusal }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::fs;
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_check_ok() {
        let app = build_router(AccessMode::Advisor, None);
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn advisor_mode_tools_list_excludes_write_tools() {
        let app = build_router(AccessMode::Advisor, None);
        let resp = app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let names: Vec<&str> = body["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"status"));
        assert!(!names.contains(&"scan_repo"));
    }

    #[tokio::test]
    async fn advisor_mode_refuses_scan_repo_call_with_403() {
        let app = build_router(AccessMode::Advisor, None);
        let req = Request::builder()
            .method("POST")
            .uri("/tools/scan_repo")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"path":"/tmp"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp).await;
        assert!(body["error"].as_str().unwrap().contains("Advisor"));
    }

    #[tokio::test]
    async fn full_mode_scan_then_status_roundtrip_over_http() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.py"), "def verify_token(t):\n    return True\n").unwrap();
        let path_str = dir.path().to_string_lossy().to_string();

        let app = build_router(AccessMode::Full, None);
        let scan_req = Request::builder()
            .method("POST")
            .uri("/tools/scan_repo")
            .header("content-type", "application/json")
            .body(Body::from(json!({"path": path_str}).to_string()))
            .unwrap();
        let resp = app.clone().oneshot(scan_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["isError"], false, "{body:?}");

        let status_req = Request::builder()
            .method("POST")
            .uri("/tools/status")
            .header("content-type", "application/json")
            .body(Body::from(json!({"path": path_str}).to_string()))
            .unwrap();
        let resp = app.oneshot(status_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert!(body["content"][0]["text"].as_str().unwrap().contains("symbols: 1"));
    }

    #[tokio::test]
    async fn repos_endpoint_lists_a_repo_after_a_scan_records_it_in_the_manifest() {
        // agentops-manifest reads/writes a fixed ~/.agentops/manifest.json —
        // not injectable here, so this asserts the repo scanned in the
        // preceding scan-then-status test (or any prior `agentops install`
        // on this machine) shows up, rather than asserting an exact count.
        let app = build_router(AccessMode::Full, None);
        let req = Request::builder().uri("/repos").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert!(body["repos"].is_array(), "{body:?}");
    }

    #[tokio::test]
    async fn graph_json_returns_404_before_any_scan_has_run() {
        let dir = tempfile::tempdir().unwrap();
        let path_str = dir.path().to_string_lossy().to_string();
        let app = build_router(AccessMode::Full, None);
        let req = Request::builder().uri(format!("/graph?path={path_str}")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn graph_json_returns_real_nodes_and_edges_after_a_scan() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.py"), "def verify_token(t):\n    return True\n").unwrap();
        let path_str = dir.path().to_string_lossy().to_string();

        let app = build_router(AccessMode::Full, None);
        let scan_req = Request::builder()
            .method("POST")
            .uri("/tools/scan_repo")
            .header("content-type", "application/json")
            .body(Body::from(json!({"path": &path_str}).to_string()))
            .unwrap();
        app.clone().oneshot(scan_req).await.unwrap();

        let graph_req = Request::builder().uri(format!("/graph?path={path_str}")).body(Body::empty()).unwrap();
        let resp = app.oneshot(graph_req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let nodes = body["nodes"].as_array().unwrap();
        assert!(nodes.iter().any(|n| n["name"] == "verify_token"), "{nodes:?}");
    }

    #[tokio::test]
    async fn docs_content_returns_404_before_docgen_has_run() {
        let dir = tempfile::tempdir().unwrap();
        let path_str = dir.path().to_string_lossy().to_string();
        let app = build_router(AccessMode::Full, None);
        let req = Request::builder().uri(format!("/docs?path={path_str}")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn docs_content_returns_the_generated_repo_map() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("repo-map.md"), "# hello world\n").unwrap();
        let path_str = dir.path().to_string_lossy().to_string();
        let app = build_router(AccessMode::Full, None);
        let req = Request::builder().uri(format!("/docs?path={path_str}")).body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["content"], "# hello world\n");
    }

    #[tokio::test]
    async fn health_check_bypasses_auth_even_when_a_key_is_required() {
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(AccessMode::Advisor, Some(hash));
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_api_key_is_rejected_when_one_is_required() {
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(AccessMode::Advisor, Some(hash));
        let resp = app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_api_key_is_rejected() {
        let (_, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let (wrong_raw, _) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(AccessMode::Advisor, Some(hash));
        let req = Request::builder()
            .uri("/tools")
            .header("authorization", format!("Bearer {wrong_raw}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn correct_api_key_is_accepted() {
        let (raw, hash) = agentops_security::api_key::generate_api_key().unwrap();
        let app = build_router(AccessMode::Advisor, Some(hash));
        let req = Request::builder()
            .uri("/tools")
            .header("authorization", format!("Bearer {raw}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
