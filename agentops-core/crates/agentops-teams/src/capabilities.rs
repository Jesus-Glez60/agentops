//! Capability resolution -- turns `PERMISSIONS_MATRIX` (this crate's own
//! single source of truth for what each role can do) into a single
//! enforcement point, `has_capability`, so route gates read from the same
//! data the Roles & Permissions tab displays instead of a second,
//! independently-hardcoded `role == "admin"` check that can drift from it.

use anyhow::Result;

use crate::{TeamStore, PERMISSIONS_MATRIX, ROLES};

/// Resolves whether `user_id` (in `tenant`) currently holds `capability`.
/// Fails closed (`Ok(false)`, never an error) for: no membership, a
/// non-active membership, or a dangling `custom:` role reference (a
/// `custom_roles` row that's since been deleted out from under a
/// membership -- shouldn't happen given `delete_custom_role`'s in-use
/// guard, but resolving to "no capabilities" rather than erroring is the
/// safe failure mode either way).
pub fn has_capability(teams: &TeamStore, tenant: &str, user_id: i64, capability: &str) -> Result<bool> {
    let Some(m) = teams.get_membership(tenant, user_id)? else { return Ok(false) };
    if m.status != "active" {
        return Ok(false);
    }
    if ROLES.contains(&m.role.as_str()) {
        return Ok(PERMISSIONS_MATRIX.iter().any(|c| c.key == capability && c.allowed_roles.contains(&m.role.as_str())));
    }
    if m.role.starts_with("custom:") {
        return Ok(teams.get_custom_role(tenant, &m.role)?.is_some_and(|cr| cr.capabilities.iter().any(|c| c == capability)));
    }
    Ok(false)
}

/// A role's *default* repo access before any per-user override is applied
/// -- admin and billing aren't override-editable at all (admin always has
/// full access, billing never does), matching the Team Management
/// mockup's disabled checkboxes for those two rows. Member/viewer default
/// to allowed, overridable per user per repo.
///
/// Relocated from `agentops-heavy-api/src/team.rs` (pure move, unchanged
/// behavior): both `/team/repo-access` and the session-authed `/repos`
/// filtering need identical semantics, and a second copy anywhere would be
/// exactly the drift `PERMISSIONS_MATRIX`'s own doc comment already warns
/// against.
pub fn role_default_repo_access(role: &str) -> (bool, bool) {
    match role {
        "admin" => (true, false),
        "billing" => (false, false),
        _ => (true, true),
    }
}

/// Whether `user_id` can access `repo_id` in `tenant`, combining role
/// default + per-user-per-repo override -- the single-repo-per-request
/// case the session-authed `/repos` filtering needs (unlike
/// `/team/repo-access`, which builds the whole grid via
/// `list_repo_overrides` in one pass). Fails closed for no membership or a
/// non-active one, same as `has_capability`.
///
/// A custom role's default (before any explicit override) is treated like
/// `"member"` -- allowed -- unless its own capability set doesn't include
/// `CAP_REPOS_VIEW`, in which case it defaults to no access at all. Fixed
/// roles use `role_default_repo_access` unchanged.
pub fn effective_repo_access(teams: &TeamStore, tenant: &str, user_id: i64, repo_id: &str) -> Result<bool> {
    let Some(m) = teams.get_membership(tenant, user_id)? else { return Ok(false) };
    if m.status != "active" {
        return Ok(false);
    }
    let default_allowed = if ROLES.contains(&m.role.as_str()) { role_default_repo_access(&m.role).0 } else { has_capability(teams, tenant, user_id, crate::CAP_REPOS_VIEW)? };
    Ok(teams.get_repo_override(tenant, user_id, repo_id)?.unwrap_or(default_allowed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_capability_matches_the_matrix_for_fixed_roles() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "admin").unwrap();
        store.add_member("tenant-a", 2, "member").unwrap();
        store.add_member("tenant-a", 3, "viewer").unwrap();

        assert!(has_capability(&store, "tenant-a", 1, crate::CAP_TEAM_MANAGE_ROLES).unwrap());
        assert!(!has_capability(&store, "tenant-a", 2, crate::CAP_TEAM_MANAGE_ROLES).unwrap());
        assert!(has_capability(&store, "tenant-a", 2, crate::CAP_GOTCHAS_COMMENT).unwrap());
        assert!(!has_capability(&store, "tenant-a", 3, crate::CAP_GOTCHAS_COMMENT).unwrap());
        assert!(has_capability(&store, "tenant-a", 3, crate::CAP_GOTCHAS_VIEW).unwrap());
    }

    #[test]
    fn has_capability_fails_closed_for_no_membership_suspended_membership_and_unknown_role() {
        let store = TeamStore::open_in_memory().unwrap();
        assert!(!has_capability(&store, "tenant-a", 999, crate::CAP_GOTCHAS_VIEW).unwrap(), "no membership at all");

        store.add_member("tenant-a", 1, "admin").unwrap();
        store.update_member("tenant-a", 1, None, Some("suspended")).unwrap();
        assert!(!has_capability(&store, "tenant-a", 1, crate::CAP_TEAM_MANAGE_ROLES).unwrap(), "suspended admin has no capabilities");
    }

    #[test]
    fn has_capability_resolves_a_custom_roles_own_capability_list() {
        let store = TeamStore::open_in_memory().unwrap();
        let custom = store.create_custom_role("tenant-a", "Junior Dev", "member", &[crate::CAP_GOTCHAS_VIEW], 1).unwrap();
        store.add_member("tenant-a", 2, &custom.role_key).unwrap();

        assert!(has_capability(&store, "tenant-a", 2, crate::CAP_GOTCHAS_VIEW).unwrap(), "granted by the custom role's own list");
        assert!(!has_capability(&store, "tenant-a", 2, crate::CAP_REPOS_CONNECT).unwrap(), "not in the custom role's list, unlike its cloned-from base role");
    }

    #[test]
    fn has_capability_fails_closed_for_a_dangling_custom_role_reference() {
        let store = TeamStore::open_in_memory().unwrap();
        // A dangling `custom:` reference (no backing `custom_roles` row)
        // can't be produced through the public API today -- `add_member`
        // validates via `is_valid_role` and `delete_custom_role` blocks
        // deleting an in-use role -- but exercising the fails-closed
        // resolution path is still worth locking in as documented
        // behavior, not just an assumption.
        store.conn.execute("INSERT INTO memberships (tenant, user_id, role) VALUES ('tenant-a', 1, 'custom:999')", []).unwrap();
        assert!(!has_capability(&store, "tenant-a", 1, crate::CAP_GOTCHAS_VIEW).unwrap());
    }

    #[test]
    fn role_default_repo_access_matches_the_mockups_disabled_rows() {
        assert_eq!(role_default_repo_access("admin"), (true, false));
        assert_eq!(role_default_repo_access("billing"), (false, false));
        assert_eq!(role_default_repo_access("member"), (true, true));
        assert_eq!(role_default_repo_access("viewer"), (true, true));
    }

    #[test]
    fn effective_repo_access_uses_role_default_then_override() {
        let store = TeamStore::open_in_memory().unwrap();
        store.add_member("tenant-a", 1, "member").unwrap();
        assert!(effective_repo_access(&store, "tenant-a", 1, "repo-1").unwrap(), "member defaults to allowed");

        store.set_repo_override("tenant-a", 1, "repo-1", false).unwrap();
        assert!(!effective_repo_access(&store, "tenant-a", 1, "repo-1").unwrap(), "explicit override wins");

        store.add_member("tenant-a", 2, "billing").unwrap();
        assert!(!effective_repo_access(&store, "tenant-a", 2, "repo-1").unwrap(), "billing defaults to no access");
    }

    #[test]
    fn effective_repo_access_for_a_custom_role_follows_its_own_capability_set() {
        let store = TeamStore::open_in_memory().unwrap();
        let with_view = store.create_custom_role("tenant-a", "Read Access", "member", &[crate::CAP_REPOS_VIEW], 1).unwrap();
        let without_view = store.create_custom_role("tenant-a", "No Access", "member", &[], 1).unwrap();
        store.add_member("tenant-a", 1, &with_view.role_key).unwrap();
        store.add_member("tenant-a", 2, &without_view.role_key).unwrap();

        assert!(effective_repo_access(&store, "tenant-a", 1, "repo-1").unwrap(), "has repos.view -- defaults to allowed like member");
        assert!(!effective_repo_access(&store, "tenant-a", 2, "repo-1").unwrap(), "lacks repos.view -- defaults to no access");

        // An explicit override still wins even without the capability.
        store.set_repo_override("tenant-a", 2, "repo-1", true).unwrap();
        assert!(effective_repo_access(&store, "tenant-a", 2, "repo-1").unwrap());
    }

    #[test]
    fn effective_repo_access_fails_closed_for_no_membership_or_suspended() {
        let store = TeamStore::open_in_memory().unwrap();
        assert!(!effective_repo_access(&store, "tenant-a", 999, "repo-1").unwrap());

        store.add_member("tenant-a", 1, "admin").unwrap();
        store.update_member("tenant-a", 1, None, Some("suspended")).unwrap();
        assert!(!effective_repo_access(&store, "tenant-a", 1, "repo-1").unwrap());
    }
}
