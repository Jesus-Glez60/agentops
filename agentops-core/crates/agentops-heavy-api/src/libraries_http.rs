//! Tenant-scoped REST equivalents of `docbrain-api`'s `/libraries`,
//! `/libraries/{slug}`, and tool-dispatch routes -- same fix shape as
//! `dashboard.rs`'s migration of `agentops-api`'s single-operator dashboard
//! routes: `agentops-server` used to mount `docbrain-api`'s router with one
//! global, non-tenant `SqliteDocbrainStore` opened once at process startup
//! (`docbrain_mcp::default_db_path()`, explicitly documented in that crate
//! as "the single-tenant, CLI-facing store"), so every tenant on the hosted
//! server shared one docbrain database instead of each getting their own
//! `docbrain-tenants/<tenant>.db`. These handlers resolve a fresh,
//! per-tenant store on every request instead, the same way `/mcp`'s tool
//! dispatch already does (`mcp_http::call_docbrain_tool`, reused directly
//! here for the tool-call route rather than re-implemented).
//!
//! Business logic (filtering, `has_mismatch` derivation, `used_in` mapping)
//! is ported verbatim from `docbrain-api/src/lib.rs`'s `list_libraries_json`/
//! `get_library_json` -- it's pure, `AppState`-independent logic, just
//! rebound to a freshly-opened per-tenant store instead of a shared one.

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use docbrain_graph::DocbrainStore;
use serde_json::{json, Value};

use crate::{require_session_capability, resolve_tenant, AppState, TenantQuery};

async fn tenant_and_capability(state: &AppState, user: &Option<axum::Extension<agentops_accounts::User>>, provided_tenant: Option<&str>) -> Result<String, (StatusCode, Json<Value>)> {
    let tenant = resolve_tenant(user, provided_tenant)?;
    require_session_capability(state, user, &tenant, agentops_teams::CAP_LIBRARIES_VIEW)?;
    Ok(tenant)
}

/// `GET /libraries` -- same shape as `docbrain-api::list_libraries_json`,
/// against the caller's own tenant's docbrain store.
pub(crate) async fn list_libraries_json(State(state): State<AppState>, user: Option<axum::Extension<agentops_accounts::User>>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let db_path = crate::docbrain_db_path_for_org(&state.docbrain_db_dir, Some(&tenant));
        let store = docbrain_graph::SqliteDocbrainStore::open(&db_path)?;
        let libs = store.list_libraries()?;

        // Only libraries with real ingested content -- same filter as
        // docbrain-api's original, see that file's comment for why.
        let libs: Vec<_> = libs.into_iter().filter(|lib| !lib.versions.is_empty()).collect();

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

        Ok(json!({ "libraries": libs_json }))
    })
    .await;

    match result {
        Ok(Ok(body)) => (StatusCode::OK, Json(body)),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("task panicked: {e}") }))),
    }
}

/// `GET /libraries/{slug}` -- same shape as `docbrain-api::get_library_json`.
pub(crate) async fn get_library_json(State(state): State<AppState>, user: Option<axum::Extension<agentops_accounts::User>>, AxumPath(slug): AxumPath<String>, Query(q): Query<TenantQuery>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Value>> {
        let db_path = crate::docbrain_db_path_for_org(&state.docbrain_db_dir, Some(&tenant));
        let store = docbrain_graph::SqliteDocbrainStore::open(&db_path)?;
        let Some(library) = store.get_library(&slug)? else { return Ok(None) };
        let used_in = store.repos_using_library(&slug)?;

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

        Ok(Some(json!({ "library": library, "used_in": used_in_json })))
    })
    .await;

    match result {
        Ok(Ok(Some(body))) => (StatusCode::OK, Json(body)),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such library registered" }))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("task panicked: {e}") }))),
    }
}

/// `POST /libraries/tools/{name}` -- REST wrapper around a docbrain tool
/// call (`discover_library`/`register_library`/`scrape_library`/`get_docs`/
/// etc.), tenant-scoped via `mcp_http::call_docbrain_tool` (the same
/// per-tenant-store-open idiom `/mcp` uses, not a second copy of it). No
/// `{slug}` path segment -- `discover_library`/`register_library` don't
/// operate on an already-registered library at all (they're what creates
/// one), so `slug` is just whichever JSON argument each tool's own schema
/// already expects, in the body, same as every other argument.
pub(crate) async fn call_library_tool_json(State(state): State<AppState>, user: Option<axum::Extension<agentops_accounts::User>>, AxumPath(name): AxumPath<String>, Query(q): Query<TenantQuery>, body: Option<Json<Value>>) -> (StatusCode, Json<Value>) {
    let tenant = match tenant_and_capability(&state, &user, q.tenant.as_deref()).await {
        Ok(t) => t,
        Err(e) => return e,
    };

    let arguments = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let result = crate::mcp_http::call_docbrain_tool(&state, &tenant, &name, arguments).await;
    (StatusCode::OK, Json(result))
}
