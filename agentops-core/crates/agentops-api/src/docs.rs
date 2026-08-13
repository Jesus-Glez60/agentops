//! `GET /repos/{name}/docs` — the Documentation Viewer's data source.
//! Mirrors `repos.rs`'s `spawn_blocking` + `find_by_name` pattern.
//!
//! Serves the persisted `doc_pages` row (written by `agentops-mcp`'s scan
//! pipeline, see `agentops_docgen::build_doc_page`'s caller) as-is: the
//! stored `content` column is already a serialized `agentops_docgen::DocPage`
//! JSON document, so this returns it directly as the response body rather
//! than deserializing it into a Rust struct first (that struct is
//! `Serialize`-only by design — see its own doc comment).

use std::path::PathBuf;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};

use crate::repos::find_by_name;
use crate::AppState;

pub async fn docs_json(State(state): State<AppState>, AxumPath(name): AxumPath<String>) -> (StatusCode, Json<Value>) {
    let manifest_path = state.manifest_path.clone();
    let target_name = name.clone();

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Value>> {
        let entries = agentops_manifest::list_scanned_repos_at(&manifest_path)?;
        let Some(entry) = find_by_name(&entries, &target_name) else {
            return Ok(None);
        };
        let path = PathBuf::from(&entry.path);
        if !path.exists() {
            return Ok(None);
        }

        let store = agentops_mcp::open_store(&path)?;
        let repo = agentops_mcp::repo_name(&path);

        if let Some((_generated_at, content_json)) = store.get_doc_page(&repo)? {
            let value: Value = serde_json::from_str(&content_json)?;
            return Ok(Some(value));
        }

        // No persisted row yet -- repo was scanned before this feature
        // shipped (or its scan's best-effort docgen step failed). Build a
        // heuristic-only page on the fly (no LLM call, no persistence) so
        // this endpoint never 404s for an already-scanned repo -- the next
        // real scan/rescan will populate the persisted row properly.
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
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": format!("no scanned repo named {name:?}") }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("internal task error: {e}") }))),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn manifest_path() -> std::path::PathBuf {
        tempfile::tempdir().unwrap().keep().join("manifest.json")
    }

    fn test_app(manifest_path: std::path::PathBuf) -> axum::Router {
        crate::build_router(agentops_mcp::AccessMode::Full, None, manifest_path)
    }

    #[tokio::test]
    async fn a_scanned_repo_serves_its_persisted_doc_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let manifest_path = manifest_path();
        agentops_manifest::record_scan_at(&manifest_path, dir.path()).unwrap();
        let name = agentops_mcp::repo_name(dir.path());

        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri(format!("/repos/{name}/docs")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["repo"], name);
        assert!(body["sections"].as_array().unwrap().iter().any(|s| s["id"] == "overview"));
    }

    #[tokio::test]
    async fn an_unknown_repo_name_404s() {
        let manifest_path = manifest_path();
        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri("/repos/nope/docs").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_repo_scanned_but_never_recorded_in_the_manifest_404s_same_as_an_unknown_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.py"), "def greet():\n    return 'hi'\n").unwrap();
        agentops_mcp::scan_and_persist(dir.path(), false).unwrap();
        let name = agentops_mcp::repo_name(dir.path());

        // No agentops_manifest::record_scan_at call -- the repo has real
        // graph data but the dashboard's manifest never learned about it,
        // matching rescanning_an_unknown_repo_name_404s' precedent in repos.rs.
        let manifest_path = manifest_path();
        let app = test_app(manifest_path);
        let resp = app.oneshot(Request::builder().uri(format!("/repos/{name}/docs")).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }
}
