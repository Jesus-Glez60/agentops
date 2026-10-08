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
            "'{connection_ref}' is not a repo connection id or URL for your organization -- use one of the ids/URLs from GET /repos, or call register_repo (with this repo's git remote URL, or with a local_id if it has no remote) to auto-register it as pending"
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
///
/// Exactly one of `repo_url`/`local_id` is expected to be `Some` (enforced
/// by the MCP tool's caller in `mcp_http.rs`, not re-validated here since
/// this fn already returns a plain user-facing string either way). The
/// `local_id` path exists for a repo with no git remote at all -- there's
/// nothing `normalize_repo_path` could ever accept for that case, and it
/// never will have one, so it's registered under a synthetic
/// `LOCAL_ONLY_URL_PREFIX`-prefixed `repo_url` instead of a real one. See
/// `ConnectionMethod::Discovered`'s doc comment for how this differs from
/// the "real remote, not yet human-connected" case.
pub(crate) fn register_repo(state: &AppState, tenant: &str, repo_url: Option<&str>, local_id: Option<&str>, name: Option<&str>) -> String {
    let store = state.store.lock().unwrap();
    let connections = store.list_connections(tenant).unwrap_or_default();

    if let Some(local_id) = local_id {
        // Unlike `repo_url`, which always gets its derived id passed through
        // `owner_repo.replace('/', "--")` below before it's ever used as a
        // connection id, `local_id` comes from the caller with no such
        // normalization -- and it ends up both as this connection's `id`
        // (a path component in `checkout_path`) and embedded in its
        // `repo_url`. Any MCP-reachable caller could otherwise pass path
        // separators or control characters through unchecked, so this path
        // needs its own equivalent guard rather than inheriting the other
        // path's safety for free.
        if local_id.is_empty() || local_id.len() > 128 || !local_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return "'local_id' must be 1-128 characters, using only letters, digits, '-' and '_'".to_string();
        }
        let synthetic_url = format!("{}{local_id}", agentops_repo_access::store::LOCAL_ONLY_URL_PREFIX);
        if let Some(existing) = connections.iter().find(|c| c.repo_url == synthetic_url) {
            return format!("local repo '{local_id}' is already registered (id: {}, status: {:?}).", existing.id, existing.status);
        }
        // Agents already choose a human-readable `local_id` by convention
        // (e.g. `job-hunter-<uuid>`) -- if the caller didn't pass an
        // explicit `name`, strip a trailing `-<uuid-looking-suffix>` so the
        // stored name is still something a later GitHub App connect can
        // match against, instead of leaving it `None` for every caller that
        // follows the existing convention but doesn't know about `name` yet.
        let derived_name = name.map(str::to_string).or_else(|| derive_name_from_local_id(local_id));
        return match store.create_discovered_connection(tenant, local_id, &synthetic_url, derived_name.as_deref()) {
            Ok(created) => format!(
                "Registered local-only repo (id: {}). It has no git remote, so it can never be cloned or indexed server-side -- scans and notes from this machine's own CLI will attach to this connection directly.",
                created.id
            ),
            Err(e) => format!("failed to register local repo '{local_id}': {e}"),
        };
    }

    let Some(repo_url) = repo_url else {
        return "register_repo requires either 'repo_url' (this repo's git remote) or 'local_id' (a stable id you generate, for a repo with no remote)".to_string();
    };

    if let Some(existing) = connections.iter().find(|c| c.repo_url == repo_url) {
        return format!("'{repo_url}' is already connected (id: {}, status: {:?}).", existing.id, existing.status);
    }
    if let Some(normalized) = agentops_repo_access::normalize_repo_path(repo_url) {
        if let Some(existing) = connections.iter().find(|c| agentops_repo_access::normalize_repo_path(&c.repo_url).as_deref() == Some(normalized.as_str())) {
            return format!("'{repo_url}' matches an already-connected repo (id: {}, status: {:?}).", existing.id, existing.status);
        }
    }

    let Some(owner_repo) = agentops_repo_access::normalize_repo_path(repo_url) else {
        return format!("'{repo_url}' doesn't look like a git remote URL -- if this repo has no remote at all, call register_repo again with 'local_id' instead of 'repo_url'");
    };
    // Same `owner--repo` id convention `github_app_routes`'s installation
    // connect flow uses for `full_name.replace('/', "--")` -- keeps ids
    // human-readable and consistent across every way a connection gets
    // created, not just this one.
    let id = owner_repo.replace('/', "--");
    let short_name = owner_repo.rsplit('/').next().unwrap_or(&owner_repo);

    // Never silently attach to a stale local-only stub just because its
    // stored name matches this repo's short name -- two different repos
    // (different owners/orgs) can share a short name, and a wrong-repo
    // attach would be worse than the duplicate-row bug this is meant to
    // fix. Surface the possible match in the response text instead, so a
    // human/agent can confirm it (via the web UI's merge action) rather
    // than this silently guessing.
    let suggestion = store.find_discovered_local_only_connections_by_name(tenant, short_name).unwrap_or_default();

    match store.create_discovered_connection(tenant, &id, repo_url, Some(short_name)) {
        Ok(created) => {
            let mut msg = format!("Registered '{repo_url}' as a pending connection (id: {}). Ask an admin to finish connecting it from Repositories -> Connect a repository.", created.id);
            if !suggestion.is_empty() {
                let ids: Vec<&str> = suggestion.iter().map(|c| c.id.as_str()).collect();
                msg.push_str(&format!(
                    " Note: found existing local-only connection(s) with a matching name ({}) -- these may be the same repo registered locally before this remote existed; review and merge/remove via the web UI if so.",
                    ids.join(", ")
                ));
            }
            msg
        }
        Err(e) => format!("failed to register '{repo_url}': {e}"),
    }
}

/// Best-effort fallback when a `local_id` caller doesn't pass an explicit
/// `name`: agents already follow a `<name>-<uuid>` convention for `local_id`
/// (see `register_repo`'s own doc comment, e.g. `job-hunter-D7CF44F0-0375-
/// 4567-9A3D-AEE5B1881FB8`), so strip a trailing standard 8-4-4-4-12 hex
/// UUID if present -- a plain `rsplit_once('-')` would only strip the last
/// dash-separated segment and leave most of the UUID in the "name". Returns
/// `None` (not the full `local_id`) when no such suffix is found, rather
/// than guessing a name that's actually just an opaque id (e.g. the CLI's
/// plain-hex `local_id`s) -- a wrong name would cause confusing/incorrect
/// merge suggestions later, so "no name" is safer than "wrong name."
fn derive_name_from_local_id(local_id: &str) -> Option<String> {
    const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];
    let segments: Vec<&str> = local_id.rsplitn(UUID_GROUP_LENGTHS.len() + 1, '-').collect();
    if segments.len() <= UUID_GROUP_LENGTHS.len() {
        return None;
    }
    let is_uuid_suffix = segments[..UUID_GROUP_LENGTHS.len()]
        .iter()
        .rev()
        .zip(UUID_GROUP_LENGTHS.iter())
        .all(|(segment, &expected_len)| segment.len() == expected_len && segment.chars().all(|c| c.is_ascii_hexdigit()));
    if !is_uuid_suffix {
        return None;
    }
    let name = segments[UUID_GROUP_LENGTHS.len()..].iter().rev().cloned().collect::<Vec<_>>().join("-");
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod name_derivation_tests {
    use super::derive_name_from_local_id;

    #[test]
    fn strips_a_standard_uuid_suffix_leaving_a_hyphenated_name_intact() {
        assert_eq!(derive_name_from_local_id("job-hunter-D7CF44F0-0375-4567-9A3D-AEE5B1881FB8"), Some("job-hunter".to_string()));
    }

    #[test]
    fn returns_none_for_a_plain_hex_id_with_no_uuid_shape() {
        assert_eq!(derive_name_from_local_id("d2ffb69148be99c3773c780c3516b58e"), None);
    }

    #[test]
    fn returns_none_when_the_suffix_isnt_actually_uuid_shaped() {
        assert_eq!(derive_name_from_local_id("my-repo-not-a-uuid"), None);
    }
}

/// Backs the `unregister_repo` MCP tool -- symmetry with `register_repo`:
/// an agent session should be able to clean up a connection it (or a human)
/// created, not just create one. Does **not** wipe `agentops-graph-pg` data
/// -- that requires `state.pg_store` and `spawn_blocking`, which this
/// crate's generic MCP tool-call path doesn't have access to the way
/// `delete_repo`'s REST handler does; an agent wanting a full cleanup
/// should use the REST endpoint (or the web UI's "Remove repository"
/// action) instead. This still removes the connection row itself, which is
/// the part that actually matters for "stop this from showing up as a
/// registered repo."
pub(crate) fn unregister_repo(state: &AppState, tenant: &str, id: &str) -> String {
    let store = state.store.lock().unwrap();
    match store.delete_connection(tenant, id) {
        Ok(true) => format!("Unregistered connection '{id}'. Its graph/notes data (if any) is not wiped by this call -- use the web UI's \"Remove repository\" action for a full cleanup."),
        Ok(false) => format!("No connection '{id}' found for this organization -- nothing to unregister."),
        Err(e) => format!("failed to unregister '{id}': {e}"),
    }
}
