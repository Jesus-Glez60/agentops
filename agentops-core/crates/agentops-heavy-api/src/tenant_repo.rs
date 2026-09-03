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
        return Err(format!(
            "'{connection_ref}' is not a repo connection id or URL for your organization -- use one of the ids/URLs from GET /repos, or call register_repo with this repo's git remote URL to auto-register it as pending"
        ));
    };
    Ok(checkout_path(&state.repo_checkouts_dir, tenant, &connection.id))
}

/// Backs the `register_repo` MCP tool (special-cased in `mcp_http`'s
/// `handle_tools_call`, alongside `resolve_connection_path`'s `path`
/// interception -- both need `AppState`/tenant access that
/// `agentops_mcp::call_tool`'s generic `(mode, name, arguments)` signature
/// doesn't carry). Lets an agent that finds itself in a repo AgentOps has
/// never seen register it (as a `Discovered`, `Pending` connection --
/// `ConnectionMethod::Discovered`'s own doc comment explains why never
/// `Active`) instead of hitting a dead end, without needing a human to open
/// the web UI first. Matches by exact `repo_url` first, then by the
/// shared `normalize_repo_path` (so an SSH-config-alias remote matches an
/// already-connected repo instead of creating a duplicate row for the same
/// repo).
pub(crate) fn register_repo(state: &AppState, tenant: &str, repo_url: &str) -> String {
    let store = state.store.lock().unwrap();
    let connections = store.list_connections(tenant).unwrap_or_default();

    if let Some(existing) = connections.iter().find(|c| c.repo_url == repo_url) {
        return format!("'{repo_url}' is already connected (id: {}, status: {:?}).", existing.id, existing.status);
    }
    if let Some(normalized) = agentops_repo_access::normalize_repo_path(repo_url) {
        if let Some(existing) = connections.iter().find(|c| agentops_repo_access::normalize_repo_path(&c.repo_url).as_deref() == Some(normalized.as_str())) {
            return format!("'{repo_url}' matches an already-connected repo (id: {}, status: {:?}).", existing.id, existing.status);
        }
    }

    let Some(owner_repo) = agentops_repo_access::normalize_repo_path(repo_url) else {
        return format!("'{repo_url}' doesn't look like a git remote URL -- nothing to register");
    };
    // Same `owner--repo` id convention `github_app_routes`'s installation
    // connect flow uses for `full_name.replace('/', "--")` -- keeps ids
    // human-readable and consistent across every way a connection gets
    // created, not just this one.
    let id = owner_repo.replace('/', "--");

    match store.create_discovered_connection(tenant, &id, repo_url) {
        Ok(created) => {
            format!("Registered '{repo_url}' as a pending connection (id: {}). Ask an admin to finish connecting it from Repositories -> Connect a repository.", created.id)
        }
        Err(e) => format!("failed to register '{repo_url}': {e}"),
    }
}
