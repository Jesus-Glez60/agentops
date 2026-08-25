//! The standard, provider-agnostic per-tenant enrollment mechanism for
//! integration modules (Phase 6c, 1.0+ roadmap) — Linear auto-kickoff is
//! the first consumer, not the only intended one. Any future integration
//! that needs "this account opted into this feature, here's its
//! module-specific config" should enroll here rather than inventing its
//! own boot-time-config-file/env-var mechanism the way Linear auto-kickoff
//! originally did (Phase 6/6b) before it was generalized.
//!
//! **Deliberately separate from `agentops-integrations`** (the credentials
//! vault) — that crate custodies secrets (API keys, OAuth tokens),
//! encrypted at rest; this crate custodies non-secret operational config
//! (which team, which repo, which local settings) plus an on/off switch.
//! Same separation-of-concerns split Phase 7 already drew between
//! `agentops-accounts` (identity) and `agentops-integrations` (secrets) —
//! extended here rather than blurred.
//!
//! **`config` is an opaque JSON blob to this crate**, not typed columns —
//! this store must not need a schema change every time a new module ships
//! its own config shape. Callers (each module's own REST handlers)
//! serialize/deserialize their own typed config; this crate only
//! guarantees storage, tenant-scoping, and the enroll/disenroll/list
//! lifecycle.
//!
//! Same multi-tenant SQLite family as `agentops-repo-access::ConnectionStore`/
//! `agentops-integrations::CredentialStore`/`agentops-accounts::AccountStore`
//! — one shared file with a `tenant` column, not docbrain's unrelated
//! one-file-per-tenant pattern (see this repo's own
//! `.agentops/notes/multi-tenant-docbrain-isolation-via-one-sqlite-file-per-tenant-not-a-tenantcontext.md`
//! for why those are two genuinely different patterns, not one).

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

#[derive(Debug, Clone)]
pub struct ModuleEnrollment {
    pub module_name: String,
    pub config: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

pub struct ModuleStore {
    conn: rusqlite::Connection,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS module_enrollments (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        tenant      TEXT NOT NULL,
        module_name TEXT NOT NULL,
        config      TEXT NOT NULL,
        enabled     INTEGER NOT NULL DEFAULT 1,
        created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        updated_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        UNIQUE (tenant, module_name)
    );
";

impl ModuleStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = rusqlite::Connection::open(path).context("opening module enrollments store")?;
        conn.execute_batch(SCHEMA).context("initializing module_enrollments schema")?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = rusqlite::Connection::open_in_memory().context("opening in-memory module enrollments store")?;
        conn.execute_batch(SCHEMA).context("initializing module_enrollments schema")?;
        Ok(Self { conn })
    }

    /// Upserts one tenant's enrollment in `module_name`, setting `enabled =
    /// true` and replacing any previously-stored config — a second call for
    /// the same `(tenant, module_name)` updates in place, not a duplicate
    /// row, matching the idempotent-upsert convention used everywhere else
    /// this rebuild (`store_credential`, `upsert_external_task`, node
    /// upsert).
    pub fn enroll(&self, tenant: &str, module_name: &str, config: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO module_enrollments (tenant, module_name, config, enabled, created_at, updated_at) \
             VALUES (?1, ?2, ?3, 1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP) \
             ON CONFLICT (tenant, module_name) DO UPDATE SET \
                config = excluded.config, \
                enabled = 1, \
                updated_at = CURRENT_TIMESTAMP",
            rusqlite::params![tenant, module_name, config],
        )?;
        Ok(())
    }

    /// Sets `enabled = false` — the row (and its config) is preserved for a
    /// later re-enable, not deleted. Returns `true` if a row actually
    /// existed to disable.
    pub fn disenroll(&self, tenant: &str, module_name: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE module_enrollments SET enabled = 0, updated_at = CURRENT_TIMESTAMP WHERE tenant = ?1 AND module_name = ?2",
            rusqlite::params![tenant, module_name],
        )?;
        Ok(changed > 0)
    }

    /// Every *enabled* enrollment across *all* tenants for one module — the
    /// cross-tenant scan a webhook-style receiver needs (an inbound
    /// request carries no tenant hint of its own until something matches
    /// it, the same reasoning Phase 6's `find_verified_team` already
    /// established for per-team secrets).
    pub fn list_enabled_for_module(&self, module_name: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT tenant, config FROM module_enrollments WHERE module_name = ?1 AND enabled = 1")?;
        let rows = stmt.query_map([module_name], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    /// Every module `tenant` has enrolled in, enabled or not — a future
    /// frontend's "your connected modules" view, module-agnostic.
    pub fn list_for_tenant(&self, tenant: &str) -> Result<Vec<ModuleEnrollment>> {
        let mut stmt = self.conn.prepare("SELECT module_name, config, enabled, created_at, updated_at FROM module_enrollments WHERE tenant = ?1 ORDER BY module_name")?;
        let rows = stmt.query_map([tenant], |r| {
            Ok(ModuleEnrollment { module_name: r.get(0)?, config: r.get(1)?, enabled: r.get::<_, i64>(2)? != 0, created_at: r.get(3)?, updated_at: r.get(4)? })
        })?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    pub fn get(&self, tenant: &str, module_name: &str) -> Result<Option<ModuleEnrollment>> {
        self.conn
            .query_row(
                "SELECT module_name, config, enabled, created_at, updated_at FROM module_enrollments WHERE tenant = ?1 AND module_name = ?2",
                rusqlite::params![tenant, module_name],
                |r| Ok(ModuleEnrollment { module_name: r.get(0)?, config: r.get(1)?, enabled: r.get::<_, i64>(2)? != 0, created_at: r.get(3)?, updated_at: r.get(4)? }),
            )
            .optional()
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enroll_then_get_round_trips_the_config() {
        let store = ModuleStore::open_in_memory().unwrap();
        store.enroll("tenant-a", "linear_auto_kickoff", r#"{"linear_team_id":"team-1"}"#).unwrap();

        let enrollment = store.get("tenant-a", "linear_auto_kickoff").unwrap().unwrap();
        assert_eq!(enrollment.config, r#"{"linear_team_id":"team-1"}"#);
        assert!(enrollment.enabled);
    }

    #[test]
    fn a_second_enroll_call_for_the_same_tenant_and_module_updates_in_place_not_a_duplicate() {
        let store = ModuleStore::open_in_memory().unwrap();
        store.enroll("tenant-a", "linear_auto_kickoff", r#"{"v":1}"#).unwrap();
        store.enroll("tenant-a", "linear_auto_kickoff", r#"{"v":2}"#).unwrap();

        assert_eq!(store.list_for_tenant("tenant-a").unwrap().len(), 1, "must update in place, not duplicate");
        assert_eq!(store.get("tenant-a", "linear_auto_kickoff").unwrap().unwrap().config, r#"{"v":2}"#);
    }

    #[test]
    fn disenroll_then_re_enroll_round_trips_and_preserves_config_meanwhile() {
        let store = ModuleStore::open_in_memory().unwrap();
        store.enroll("tenant-a", "linear_auto_kickoff", r#"{"v":1}"#).unwrap();

        assert!(store.disenroll("tenant-a", "linear_auto_kickoff").unwrap());
        let disabled = store.get("tenant-a", "linear_auto_kickoff").unwrap().unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.config, r#"{"v":1}"#, "config must survive being disabled, not get wiped");

        assert!(store.list_enabled_for_module("linear_auto_kickoff").unwrap().is_empty(), "a disabled enrollment must not show up as enabled");

        store.enroll("tenant-a", "linear_auto_kickoff", r#"{"v":1}"#).unwrap();
        assert!(store.get("tenant-a", "linear_auto_kickoff").unwrap().unwrap().enabled, "re-enrolling must re-enable");
    }

    #[test]
    fn disenroll_reports_false_when_nothing_was_enrolled() {
        let store = ModuleStore::open_in_memory().unwrap();
        assert!(!store.disenroll("tenant-a", "linear_auto_kickoff").unwrap());
    }

    #[test]
    fn one_tenant_never_sees_another_tenants_enrollments() {
        let store = ModuleStore::open_in_memory().unwrap();
        store.enroll("tenant-a", "linear_auto_kickoff", r#"{"v":1}"#).unwrap();

        assert!(store.get("tenant-b", "linear_auto_kickoff").unwrap().is_none());
        assert!(store.list_for_tenant("tenant-b").unwrap().is_empty());
    }

    #[test]
    fn list_enabled_for_module_scans_across_every_tenant() {
        let store = ModuleStore::open_in_memory().unwrap();
        store.enroll("tenant-a", "linear_auto_kickoff", r#"{"v":"a"}"#).unwrap();
        store.enroll("tenant-b", "linear_auto_kickoff", r#"{"v":"b"}"#).unwrap();
        store.enroll("tenant-a", "some_other_module", r#"{"v":"other"}"#).unwrap();

        let enabled = store.list_enabled_for_module("linear_auto_kickoff").unwrap();
        assert_eq!(enabled.len(), 2, "{enabled:?}");
        let tenants: std::collections::HashSet<_> = enabled.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(tenants, std::collections::HashSet::from(["tenant-a", "tenant-b"]));
    }

    #[test]
    fn list_for_tenant_covers_every_module_that_tenant_has_ever_enrolled_in() {
        let store = ModuleStore::open_in_memory().unwrap();
        store.enroll("tenant-a", "linear_auto_kickoff", r#"{}"#).unwrap();
        store.enroll("tenant-a", "some_other_module", r#"{}"#).unwrap();

        let listed = store.list_for_tenant("tenant-a").unwrap();
        assert_eq!(listed.len(), 2);
    }
}
