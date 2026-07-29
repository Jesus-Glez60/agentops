//! REST API server (axum) wrapping the exact same tool logic as
//! `agentops-mcp` — `list_tools`/`call_tool` are reused directly rather than
//! re-implemented, so there's one implementation of each operation (scan,
//! note-taking, docgen) behind two transports, not two. `AccessMode`
//! enforcement is therefore identical to the MCP server: a client hitting
//! `GET /tools` in Advisor mode gets back exactly the 3 read-only tool
//! definitions, and `POST /tools/scan_repo` in Advisor mode is refused before
//! any repo-scanning code runs.

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

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
        .layer(middleware::from_fn_with_state(state.clone(), require_api_key))
        .route("/health", get(health))
        .with_state(state)
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
