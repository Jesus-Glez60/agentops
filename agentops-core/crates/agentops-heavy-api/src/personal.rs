//! The personal connection (milestone 5 item 4): one per user account,
//! holding memories about the person rather than any repo, recalled in
//! every repo they work in.
//!
//! It lives in its own tenant, `u<user_id>`, never in the user's org
//! tenant: an org can have several members (and invites move a user
//! between orgs), and every org-scoped lookup, listing, dashboard and
//! reindex resolves against the org tenant, so none of them can see it,
//! even given its id. It is only ever reached from the caller's own user
//! id: the `path` alias `"personal"` on `/mcp`, and the hook route's
//! authenticated user. It follows the user across org switches.

use std::path::PathBuf;

use crate::indexing::{checkout_path, notes_dir_for_checkout};
use crate::AppState;

/// The `path` value that names the caller's personal connection.
pub(crate) const PERSONAL_ALIAS: &str = "personal";

/// The personal connection's id inside its tenant.
const PERSONAL_CONNECTION_ID: &str = "personal";

/// The tenant holding user `user_id`'s personal connection. An org tenant
/// is 32 hex characters (`new_random_id`), so `u<digits>` never collides.
pub(crate) fn personal_tenant(user_id: i64) -> String {
    format!("u{user_id}")
}

/// The checkout path of `user_id`'s personal connection, whose name is its
/// store repo. `None` when it doesn't exist and `create` is false: reading
/// never creates it, only a write does (`add_note`, the import). Creating
/// registers it like any local-only connection (`register_repo` with a
/// `local_id`) and makes its empty checkout and notes folders, without
/// which `repo_name` can't canonicalize the path (gotcha 326953).
pub(crate) fn personal_path(state: &AppState, user_id: i64, create: bool) -> Result<Option<PathBuf>, String> {
    let tenant = personal_tenant(user_id);
    let exists = state.store.lock().unwrap().get_connection(&tenant, PERSONAL_CONNECTION_ID).map_err(|e| e.to_string())?.is_some();
    if !exists {
        if !create {
            return Ok(None);
        }
        let message = crate::tenant_repo::register_repo(state, &tenant, None, Some(PERSONAL_CONNECTION_ID), Some(PERSONAL_ALIAS));
        if !message.starts_with("Registered") && !message.contains("already registered") {
            return Err(format!("creating your personal connection: {message}"));
        }
    }
    let path = checkout_path(&state.repo_checkouts_dir, &tenant, PERSONAL_CONNECTION_ID);
    for dir in [path.clone(), notes_dir_for_checkout(&path)] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    Ok(Some(path))
}
