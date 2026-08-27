//! Shared tenant -> checkout-path resolution, used by every route that
//! needs to turn a caller's bearer token + a client-supplied connection
//! reference into a real filesystem path scoped to that caller's own
//! tenant. Originally built for `/mcp` (see `mcp_http`'s module doc
//! comment for the full threat-model rationale -- unchanged here, just
//! relocated so the dashboard-unification routes in `lib.rs` can reuse it
//! instead of duplicating it).

use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::indexing::checkout_path;
use crate::AppState;

/// The caller's resolved tenant for a tenant-scoped request -- deliberately
/// just this one field, not the full `User` `require_api_key_or_session`
/// inserts for every other route in this crate. Handlers using this only
/// ever need a tenant to scope `ConnectionStore` lookups; keeping this
/// separate from `User` also keeps a personal API key's reach scoped to
/// exactly the routes gated by `require_tenant_auth` rather than silently
/// working against every other `require_api_key_or_session`-gated route
/// (repo connect, GitHub App installs, etc.) -- a personal key minted for
/// "connect my coding tool" shouldn't double as a general
/// account/repo-management credential.
#[derive(Clone)]
pub(crate) struct TenantCaller {
    pub(crate) tenant: String,
}

/// Session-first, then a per-user API key -- **not** the instance-wide
/// `AGENTOPS_API_KEY_HASH` (unlike `require_api_key_or_session`). That key
/// carries no tenant, and every route gated by this middleware rests on
/// resolving a request against the caller's own tenant's connections --
/// there's no safe fallback for a caller with no tenant at all short of
/// trusting a client-supplied literal path/id, which is exactly what this
/// exists to avoid for a network-reachable endpoint.
pub(crate) async fn require_tenant_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(token) = req.headers().get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) else {
        return unauthorized();
    };
    // Resolved in a plain sync fn, never a `MutexGuard` anywhere in this
    // `async fn`'s own body -- `std::sync::MutexGuard` isn't `Send`, and
    // `middleware::from_fn_with_state` requires the whole future this
    // function produces to be `Send`. The guard here is always dropped
    // before any `.await` regardless (both branches return early), but
    // rustc's async state-machine transform doesn't reliably prove that on
    // its own; moving the lookup out of the `async fn` sidesteps the
    // question entirely instead of fighting the borrow checker over it.
    let Some(caller) = resolve_tenant_caller(&state, token) else {
        return unauthorized();
    };
    req.extensions_mut().insert(caller);
    next.run(req).await
}

fn resolve_tenant_caller(state: &AppState, token: &str) -> Option<TenantCaller> {
    let accounts = state.accounts.as_ref()?;
    let accounts = accounts.lock().unwrap();
    if let Ok(user) = accounts.verify_session(token) {
        return Some(TenantCaller { tenant: user.tenant });
    }
    if let Ok(Some((_user_id, tenant))) = accounts.verify_user_api_key(token) {
        return Some(TenantCaller { tenant });
    }
    None
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing or invalid credentials" }))).into_response()
}

/// `connection_ref` must name a `RepoConnection` (by id or its `repo_url`)
/// belonging to `tenant` -- anything else is rejected, never treated as a
/// literal filesystem path. See `mcp_http`'s module doc comment for why.
pub(crate) fn resolve_connection_path(state: &AppState, tenant: &str, connection_ref: &str) -> Result<std::path::PathBuf, String> {
    let store = state.store.lock().unwrap();
    let connection = store
        .get_connection(tenant, connection_ref)
        .ok()
        .flatten()
        .or_else(|| store.list_connections(tenant).ok()?.into_iter().find(|c| c.repo_url == connection_ref));
    let Some(connection) = connection else {
        return Err(format!("'{connection_ref}' is not a repo connection id or URL for your organization -- use one of the ids/URLs from GET /repos"));
    };
    Ok(checkout_path(&state.repo_checkouts_dir, tenant, &connection.id))
}
