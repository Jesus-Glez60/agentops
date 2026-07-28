//! REST API server (axum) wrapping the exact same tool logic as
//! `agentops-mcp` — `list_tools`/`call_tool` are reused directly rather than
//! re-implemented, so there's one implementation of each operation (scan,
//! note-taking, docgen) behind two transports, not two. `AccessMode`
//! enforcement is therefore identical to the MCP server: a client hitting
//! `GET /tools` in Advisor mode gets back exactly the 3 read-only tool
//! definitions, and `POST /tools/scan_repo` in Advisor mode is refused before
//! any repo-scanning code runs.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use agentops_mcp::AccessMode;

/// Builds the router for a server running in `mode`. `AccessMode` is fixed
/// for the lifetime of the server (mirroring the MCP server's design) rather
/// than something a client can request per-call.
pub fn build_router(mode: AccessMode) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/tools", get(list_tools_handler))
        .route("/tools/{name}", post(call_tool_handler))
        .with_state(mode)
}

/// Binds `addr` and serves until the process is killed.
pub async fn run(addr: &str, mode: AccessMode) -> anyhow::Result<()> {
    let app = build_router(mode);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("agentops-api listening on {addr} (access mode: {mode:?})");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn list_tools_handler(State(mode): State<AccessMode>) -> Json<Value> {
    let tools = agentops_mcp::list_tools(mode);
    Json(json!({ "tools": tools }))
}

async fn call_tool_handler(
    State(mode): State<AccessMode>,
    Path(name): Path<String>,
    body: Option<Json<Value>>,
) -> (StatusCode, Json<Value>) {
    let empty = json!({});
    let args = body.map(|Json(v)| v).unwrap_or(empty);

    match agentops_mcp::call_tool(mode, &name, &args) {
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
        let app = build_router(AccessMode::Advisor);
        let resp = app.oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn advisor_mode_tools_list_excludes_write_tools() {
        let app = build_router(AccessMode::Advisor);
        let resp = app.oneshot(Request::builder().uri("/tools").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        let names: Vec<&str> = body["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"status"));
        assert!(!names.contains(&"scan_repo"));
    }

    #[tokio::test]
    async fn advisor_mode_refuses_scan_repo_call_with_403() {
        let app = build_router(AccessMode::Advisor);
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

        let app = build_router(AccessMode::Full);
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
}
