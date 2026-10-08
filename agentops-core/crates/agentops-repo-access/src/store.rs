//! Persists repo connections — SQLite-backed, same architectural pattern as
//! `docbrain-graph`'s `DocbrainStore`: **every read/write method requires a
//! tenant string as its first argument**, so a query that forgets to scope
//! by tenant is a compile error, not a runtime bug that leaks one tenant's
//! connection records (including public keys and encrypted private key
//! blobs) to another.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionMethod {
    Ssh,
    GitHubApp,
    /// Either of two cases, distinguished by `repo_url`'s prefix
    /// (`is_local_only_url`):
    /// - An agent (via the `register_repo` MCP tool) found this repo's git
    ///   remote mentioned during a session, but no human has granted real
    ///   access yet -- no keypair, no App installation, nothing in
    ///   `public_key_openssh`/`encrypted_private_key_openssh`/
    ///   `installation_id`. A human finishes connecting it via the same
    ///   `/repositories/connect` wizard as any other repo, which replaces
    ///   this row's method entirely once real auth material exists.
    /// - A repo with no git remote at all, registered via `register_repo`'s
    ///   `local_id` path with a synthetic `LOCAL_ONLY_URL_PREFIX`-prefixed
    ///   `repo_url`. This is a *permanent* state, not "pending" -- there is
    ///   no remote to eventually connect via SSH/GitHub App, ever. Notes and
    ///   scans for it come entirely from the registering machine's own CLI
    ///   (`scan_repo`/`add_note` against this connection's id), never a
    ///   server-side clone.
    /// Always created `Pending` (see `create_discovered_connection`).
    Discovered,
}

/// Prefix used for the synthetic `repo_url` of a `Discovered` connection
/// registered for a repo with no git remote at all (see `register_repo`'s
/// `local_id` path in `agentops-heavy-api`). Never a real clonable URL --
/// callers that need to clone/index a connection must check for this
/// prefix first and treat it as permanently local-only, not "pending, needs
/// a human to finish connecting" like every other `Discovered` row.
pub const LOCAL_ONLY_URL_PREFIX: &str = "local:";

pub fn is_local_only_url(repo_url: &str) -> bool {
    repo_url.starts_with(LOCAL_ONLY_URL_PREFIX)
}

/// A stable id for a repo with no git remote, generated client-side (by
/// `agentops-cli`) and passed as `register_repo`'s `local_id` argument --
/// same random-hex-string shape `agentops-heavy-api::indexing`'s
/// `new_random_job_id` already uses, exposed here so the CLI doesn't need
/// its own copy of the same four lines.
pub fn new_local_repo_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system randomness must be available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl ConnectionMethod {
    fn as_str(&self) -> &'static str {
        match self {
            ConnectionMethod::Ssh => "ssh",
            ConnectionMethod::GitHubApp => "github_app",
            ConnectionMethod::Discovered => "discovered",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ssh" => Ok(ConnectionMethod::Ssh),
            "github_app" => Ok(ConnectionMethod::GitHubApp),
            "discovered" => Ok(ConnectionMethod::Discovered),
            other => anyhow::bail!("unknown connection method {other:?}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    /// Deploy key generated (or App install link issued) but not yet
    /// confirmed working — e.g. the operator hasn't pasted the public key
    /// into GitHub's Deploy Keys UI yet.
    Pending,
    /// A clone/fetch (or installation-token exchange) has succeeded at
    /// least once.
    Active,
    Failed(String),
}

impl ConnectionStatus {
    fn as_db_string(&self) -> String {
        match self {
            ConnectionStatus::Pending => "pending".to_string(),
            ConnectionStatus::Active => "active".to_string(),
            ConnectionStatus::Failed(reason) => format!("failed:{reason}"),
        }
    }

    fn from_db_string(s: &str) -> Self {
        match s {
            "pending" => ConnectionStatus::Pending,
            "active" => ConnectionStatus::Active,
            other => match other.strip_prefix("failed:") {
                Some(reason) => ConnectionStatus::Failed(reason.to_string()),
                None => ConnectionStatus::Failed(other.to_string()),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConnection {
    pub id: String,
    pub tenant: String,
    pub repo_url: String,
    pub method: ConnectionMethod,
    /// `Some` only for `ConnectionMethod::Ssh` — safe to display.
    pub public_key_openssh: Option<String>,
    /// `Some` only for `ConnectionMethod::Ssh` — already encrypted, safe to
    /// persist, but never rendered to a UI or logged.
    pub encrypted_private_key_openssh: Option<String>,
    /// `Some` only for `ConnectionMethod::GitHubApp` — the installation
    /// this connection was created from, joined against
    /// `indexing_store::GitHubAppInstallation` to resolve which tenant the
    /// connection belongs to.
    pub installation_id: Option<String>,
    pub status: ConnectionStatus,
    pub created_at: String,
    /// User-selected branch override for indexing -- `None` means "no
    /// override, index whatever the clone's default branch is." Distinct
    /// from the dashboard's separately live-read `branch` field (what's
    /// actually checked out right now): this is the *intent*, that's the
    /// *observed state*.
    pub tracked_branch: Option<String>,
    /// Short human-readable name (e.g. a repo's basename or short GitHub
    /// name), used only to match a `local:`-prefixed `Discovered` connection
    /// against a real repo being connected later -- `local:<id>` carries no
    /// information a real `repo_url`/`full_name` can be compared against, so
    /// this is the one piece of identity that survives both sides. `None`
    /// for connections created before this field existed, or created via a
    /// path that never supplied one; such rows simply never match.
    pub name: Option<String>,
}

pub struct ConnectionStore {
    conn: Connection,
}

impl ConnectionStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("opening connection-store database")?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory connection-store database")?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS repo_connections (
                id                          TEXT NOT NULL,
                tenant                      TEXT NOT NULL,
                repo_url                    TEXT NOT NULL,
                method                      TEXT NOT NULL,
                public_key_openssh          TEXT,
                encrypted_private_key_openssh TEXT,
                installation_id             TEXT,
                status                      TEXT NOT NULL,
                created_at                  TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (tenant, id)
            )",
            [],
        )
        .context("creating repo_connections table")?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_repo_connections_tenant ON repo_connections(tenant)", [])
            .context("creating tenant index")?;
        // `ALTER TABLE ... ADD COLUMN` is a no-op error (not silently
        // ignored) against a table that already has the column -- this
        // covers a database file created before `installation_id` existed,
        // matching `CREATE TABLE IF NOT EXISTS`'s own "don't error on an
        // already-migrated file" posture for genuinely new deployments.
        let _ = conn.execute("ALTER TABLE repo_connections ADD COLUMN installation_id TEXT", []);
        let _ = conn.execute("ALTER TABLE repo_connections ADD COLUMN tracked_branch TEXT", []);
        let _ = conn.execute("ALTER TABLE repo_connections ADD COLUMN name TEXT", []);
        conn.execute("CREATE INDEX IF NOT EXISTS idx_repo_connections_tenant_name ON repo_connections(tenant, name)", [])
            .context("creating tenant/name index")?;
        Ok(Self { conn })
    }

    /// Records a new SSH-deploy-key connection. `id` should be the same
    /// `repo_id` passed to `generate_deploy_keypair_for_repo` (the
    /// `SecretsProvider` scoping key), not a fresh random value — the
    /// passphrase derivation and the stored record must agree on it.
    pub fn create_ssh_connection(&self, tenant: &str, id: &str, repo_url: &str, keypair: &crate::DeployKeypair) -> Result<RepoConnection> {
        self.conn
            .execute(
                "INSERT INTO repo_connections (id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    id,
                    tenant,
                    repo_url,
                    ConnectionMethod::Ssh.as_str(),
                    keypair.public_key_openssh,
                    keypair.encrypted_private_key_openssh,
                    ConnectionStatus::Pending.as_db_string(),
                ],
            )
            .context("inserting repo connection")?;
        self.get_connection(tenant, id)?.context("just-inserted connection not found — this is a store bug")
    }

    /// Records a new GitHub-App-backed connection. Unlike SSH, there's no
    /// private key to custody and no separate verify step -- the
    /// installation-token exchange the caller already performed (to list
    /// the installation's repos in the first place) is itself proof the
    /// App has real access, so this starts `Active` immediately rather than
    /// `Pending`.
    pub fn create_github_app_connection(&self, tenant: &str, id: &str, repo_url: &str, installation_id: &str) -> Result<RepoConnection> {
        self.conn
            .execute(
                "INSERT INTO repo_connections (id, tenant, repo_url, method, installation_id, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, tenant, repo_url, ConnectionMethod::GitHubApp.as_str(), installation_id, ConnectionStatus::Active.as_db_string()],
            )
            .context("inserting github app repo connection")?;
        self.get_connection(tenant, id)?.context("just-inserted connection not found — this is a store bug")
    }

    /// Records a repo an agent found mentioned (a git remote with no
    /// existing connection) via the `register_repo` MCP tool. Always starts
    /// `Pending`, like `create_ssh_connection` — never `Active` like
    /// `create_github_app_connection`, since unlike an App installation-
    /// token exchange, nothing here proves real access: no keypair was
    /// generated, no App was installed, an agent just reported seeing this
    /// URL. A human must still finish connecting it (SSH or GitHub App)
    /// before anything can actually clone/index it.
    pub fn create_discovered_connection(&self, tenant: &str, id: &str, repo_url: &str, name: Option<&str>) -> Result<RepoConnection> {
        self.conn
            .execute(
                "INSERT INTO repo_connections (id, tenant, repo_url, method, status, name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, tenant, repo_url, ConnectionMethod::Discovered.as_str(), ConnectionStatus::Pending.as_db_string(), name],
            )
            .context("inserting discovered repo connection")?;
        self.get_connection(tenant, id)?.context("just-inserted connection not found — this is a store bug")
    }

    /// Overwrites an SSH connection's keypair in place (status reset to
    /// `Pending`, same as a brand-new connection) -- the failure screen's
    /// "Regenerate deploy key" action, for when the previously-issued key
    /// was removed or rotated out from under an already-connected repo.
    pub fn replace_ssh_keypair(&self, tenant: &str, id: &str, keypair: &crate::DeployKeypair) -> Result<RepoConnection> {
        let updated = self
            .conn
            .execute(
                "UPDATE repo_connections SET public_key_openssh = ?1, encrypted_private_key_openssh = ?2, status = ?3
                 WHERE tenant = ?4 AND id = ?5 AND method = ?6",
                rusqlite::params![
                    keypair.public_key_openssh,
                    keypair.encrypted_private_key_openssh,
                    ConnectionStatus::Pending.as_db_string(),
                    tenant,
                    id,
                    ConnectionMethod::Ssh.as_str(),
                ],
            )
            .context("replacing ssh keypair")?;
        if updated == 0 {
            anyhow::bail!("no SSH connection {id:?} for tenant {tenant:?} — refusing a silent no-op update");
        }
        self.get_connection(tenant, id)?.context("just-updated connection not found — this is a store bug")
    }

    /// Removes exactly one connection (tenant + id scoped). Returns whether
    /// a row was actually removed, so a caller can tell "already gone" from
    /// "just deleted" without a separate existence check. Scoped delete,
    /// unlike `delete_all_for_tenant` (org deletion's full wipe) -- this is
    /// the single-connection "unregister" path that had no code path at all
    /// before this.
    pub fn delete_connection(&self, tenant: &str, id: &str) -> Result<bool> {
        let deleted = self
            .conn
            .execute("DELETE FROM repo_connections WHERE tenant = ?1 AND id = ?2", rusqlite::params![tenant, id])
            .context("deleting repo connection")?;
        Ok(deleted > 0)
    }

    /// Checks whether `repo_url` (after host-agnostic normalization) already
    /// belongs to a *different* connection for this tenant -- the same
    /// dedup check `register_repo` already does before creating a new
    /// `Discovered` connection (`agentops-heavy-api::tenant_repo`), reused
    /// here so attaching a remote can't silently create two connections
    /// pointing at the same repo.
    fn find_other_connection_with_repo_url(&self, tenant: &str, excluding_id: &str, repo_url: &str) -> Result<Option<RepoConnection>> {
        let Some(normalized) = crate::normalize_repo_path(repo_url) else { return Ok(None) };
        for connection in self.list_connections(tenant)? {
            if connection.id == excluding_id {
                continue;
            }
            if crate::normalize_repo_path(&connection.repo_url).as_deref() == Some(normalized.as_str()) {
                return Ok(Some(connection));
            }
        }
        Ok(None)
    }

    /// Shared guard for both `attach_*` methods: loads the existing
    /// connection, and refuses (with a clear, specific reason) to touch
    /// anything that isn't actually eligible to be upgraded. Eligible means
    /// `method == Discovered` -- covers both sub-cases that enum variant
    /// represents (a true local-only placeholder, and a real-but-not-yet-
    /// authenticated repo `register_repo` already found) in one condition,
    /// and naturally excludes an already-`Ssh`/`GitHubApp` connection (not a
    /// supported operation -- that connection already has a real remote and
    /// working auth material, attaching a different one isn't "completing"
    /// it, it's silently repointing it).
    fn load_attachable_connection(&self, tenant: &str, id: &str, new_repo_url: &str) -> Result<RepoConnection> {
        let existing = self.get_connection(tenant, id)?.with_context(|| format!("no connection {id:?} for tenant {tenant:?}"))?;
        if existing.method != ConnectionMethod::Discovered {
            anyhow::bail!("connection {id:?} is already {:?} — only a pending/unconnected connection can be upgraded", existing.method);
        }
        if !is_local_only_url(&existing.repo_url) {
            // Already has a real repo_url (the "register_repo found a real
            // remote, nobody's connected it yet" sub-case) -- attaching must
            // be completing *that* connection, not repointing it to a
            // different repo.
            let existing_normalized = crate::normalize_repo_path(&existing.repo_url);
            let new_normalized = crate::normalize_repo_path(new_repo_url);
            if existing_normalized != new_normalized {
                anyhow::bail!("connection {id:?} already points at {:?} — attaching a different remote isn't supported", existing.repo_url);
            }
        }
        if let Some(other) = self.find_other_connection_with_repo_url(tenant, id, new_repo_url)? {
            anyhow::bail!("repo_url {new_repo_url:?} is already connected as {:?} — refusing to create a duplicate", other.id);
        }
        Ok(existing)
    }

    /// Attaches a real SSH deploy-key remote to a connection that's
    /// currently `Discovered` (local-only, or real-url-but-unauthenticated)
    /// -- turns it into a working `Ssh` connection in place, same `id`, so
    /// every gotcha/decision/doc already recorded under that id stays
    /// attached. See `load_attachable_connection` for the eligibility guard.
    pub fn attach_ssh_remote(&self, tenant: &str, id: &str, repo_url: &str, keypair: &crate::DeployKeypair) -> Result<RepoConnection> {
        self.load_attachable_connection(tenant, id, repo_url)?;
        self.conn
            .execute(
                "UPDATE repo_connections SET repo_url = ?1, method = ?2, public_key_openssh = ?3, encrypted_private_key_openssh = ?4, status = ?5
                 WHERE tenant = ?6 AND id = ?7",
                rusqlite::params![
                    repo_url,
                    ConnectionMethod::Ssh.as_str(),
                    keypair.public_key_openssh,
                    keypair.encrypted_private_key_openssh,
                    ConnectionStatus::Pending.as_db_string(),
                    tenant,
                    id,
                ],
            )
            .context("attaching ssh remote")?;
        self.get_connection(tenant, id)?.context("just-updated connection not found — this is a store bug")
    }

    /// Attaches a real GitHub App remote to a connection that's currently
    /// `Discovered` -- same shape as `attach_ssh_remote`, see
    /// `load_attachable_connection` for the eligibility guard. Starts
    /// `Active` immediately, same reasoning as `create_github_app_connection`
    /// (the installation-token exchange the caller already performed is
    /// itself proof of real access).
    pub fn attach_github_app_remote(&self, tenant: &str, id: &str, repo_url: &str, installation_id: &str) -> Result<RepoConnection> {
        self.load_attachable_connection(tenant, id, repo_url)?;
        self.conn
            .execute(
                "UPDATE repo_connections SET repo_url = ?1, method = ?2, installation_id = ?3, status = ?4
                 WHERE tenant = ?5 AND id = ?6",
                rusqlite::params![repo_url, ConnectionMethod::GitHubApp.as_str(), installation_id, ConnectionStatus::Active.as_db_string(), tenant, id],
            )
            .context("attaching github app remote")?;
        self.get_connection(tenant, id)?.context("just-updated connection not found — this is a store bug")
    }

    pub fn get_connection(&self, tenant: &str, id: &str) -> Result<Option<RepoConnection>> {
        self.conn
            .query_row(
                "SELECT id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, installation_id, status, created_at, tracked_branch, name
                 FROM repo_connections WHERE tenant = ?1 AND id = ?2",
                rusqlite::params![tenant, id],
                row_to_connection,
            )
            .map(Some)
            .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })
            .context("querying repo connection")
    }

    pub fn list_connections(&self, tenant: &str) -> Result<Vec<RepoConnection>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, installation_id, status, created_at, tracked_branch, name
                 FROM repo_connections WHERE tenant = ?1 ORDER BY created_at DESC",
            )
            .context("preparing list query")?;
        let rows = stmt.query_map([tenant], row_to_connection).context("querying repo connections")?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("reading repo connection rows")
    }

    /// Every `Discovered`, `local:`-prefixed connection for `tenant` whose
    /// stored `name` matches `name` case-insensitively -- used by
    /// `connect_from_installation`/`register_repo`'s `repo_url` path to
    /// *suggest* (never silently perform) a merge with a stale local-only
    /// stub when a real repo is connected later. Deliberately never used to
    /// auto-attach: two differently-owned GitHub repos can share a short
    /// name, so a single match here is a suggestion, not proof of identity.
    pub fn find_discovered_local_only_connections_by_name(&self, tenant: &str, name: &str) -> Result<Vec<RepoConnection>> {
        Ok(self
            .list_connections(tenant)?
            .into_iter()
            .filter(|c| c.method == ConnectionMethod::Discovered && is_local_only_url(&c.repo_url))
            .filter(|c| c.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .collect())
    }

    /// Finds the connection, scoped to `tenant`, whose `repo_url` normalizes
    /// to the same repo as `repo_url` -- the read-side
    /// counterpart to `find_other_connection_with_repo_url` (which exists
    /// purely as a write-time dedup guard and takes an `excluding_id` built
    /// for that purpose). Used by the GitHub webhook handler to find a
    /// connection whose id no longer matches the payload-derived
    /// `owner--repo` id, because it was upgraded in place via `attach_*` and
    /// kept its original id.
    pub fn find_connection_by_repo_url(&self, tenant: &str, repo_url: &str) -> Result<Option<RepoConnection>> {
        let Some(normalized) = crate::normalize_repo_path(repo_url) else { return Ok(None) };
        for connection in self.list_connections(tenant)? {
            if crate::normalize_repo_path(&connection.repo_url).as_deref() == Some(normalized.as_str()) {
                return Ok(Some(connection));
            }
        }
        Ok(None)
    }

    /// Every connection created from a given GitHub App installation --
    /// used by the webhook receiver's `installation`(deleted)/
    /// `installation_repositories`(removed) handling to find which
    /// connections to mark failed, without the tenant needing to be known
    /// up front (the caller resolves it from `tenant_for_installation`
    /// first, then calls this).
    pub fn connections_for_installation(&self, tenant: &str, installation_id: &str) -> Result<Vec<RepoConnection>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, installation_id, status, created_at, tracked_branch, name
                 FROM repo_connections WHERE tenant = ?1 AND installation_id = ?2",
            )
            .context("preparing installation connections query")?;
        let rows = stmt.query_map(rusqlite::params![tenant, installation_id], row_to_connection).context("querying installation connections")?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("reading installation connection rows")
    }

    pub fn set_status(&self, tenant: &str, id: &str, status: ConnectionStatus) -> Result<()> {
        let updated = self
            .conn
            .execute(
                "UPDATE repo_connections SET status = ?1 WHERE tenant = ?2 AND id = ?3",
                rusqlite::params![status.as_db_string(), tenant, id],
            )
            .context("updating repo connection status")?;
        if updated == 0 {
            anyhow::bail!("no connection {id:?} for tenant {tenant:?} — refusing a silent no-op update");
        }
        Ok(())
    }

    /// Sets (or, with `None`, clears) `id`'s indexing branch override. Purely
    /// a persistence step — the caller is responsible for re-indexing
    /// afterward if it wants the new branch actually checked out.
    pub fn set_tracked_branch(&self, tenant: &str, id: &str, branch: Option<&str>) -> Result<()> {
        let updated = self
            .conn
            .execute("UPDATE repo_connections SET tracked_branch = ?1 WHERE tenant = ?2 AND id = ?3", rusqlite::params![branch, tenant, id])
            .context("updating repo connection tracked branch")?;
        if updated == 0 {
            anyhow::bail!("no connection {id:?} for tenant {tenant:?} — refusing a silent no-op update");
        }
        Ok(())
    }

    /// Deletes every repo connection for `tenant` -- the leaf step of the
    /// org-deletion cascade (`POST /team/delete-organization`). Returns the
    /// number of rows removed, purely informational (the caller doesn't
    /// branch on it -- unlike `set_status`, zero deleted rows is a normal
    /// outcome here, not a signal something went wrong).
    pub fn delete_all_for_tenant(&self, tenant: &str) -> Result<usize> {
        self.conn.execute("DELETE FROM repo_connections WHERE tenant = ?1", [tenant]).context("deleting all repo connections for tenant")
    }
}

fn row_to_connection(row: &rusqlite::Row) -> rusqlite::Result<RepoConnection> {
    let method_str: String = row.get(3)?;
    let status_str: String = row.get(7)?;
    Ok(RepoConnection {
        id: row.get(0)?,
        tenant: row.get(1)?,
        repo_url: row.get(2)?,
        method: ConnectionMethod::from_str(&method_str).map_err(|e| rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.into()))?,
        public_key_openssh: row.get(4)?,
        encrypted_private_key_openssh: row.get(5)?,
        installation_id: row.get(6)?,
        status: ConnectionStatus::from_db_string(&status_str),
        created_at: row.get(8)?,
        tracked_branch: row.get(9)?,
        name: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate_deploy_keypair_for_repo;
    use crate::secrets::EnvSecretsProvider;

    fn test_store() -> ConnectionStore {
        ConnectionStore::open_in_memory().unwrap()
    }

    fn test_keypair(tenant: &str, id: &str) -> crate::DeployKeypair {
        let provider = EnvSecretsProvider::from_hex(&"11".repeat(32)).unwrap();
        generate_deploy_keypair_for_repo(&provider, tenant, id).unwrap()
    }

    /// Regression test for the exact class of bug
    /// `create-table-if-not-exists-does-not-migrate-existing-sqlite-schemas.md`
    /// already documents against `agentops-accounts`: build a
    /// pre-`installation_id`-column table by hand (bypassing
    /// `from_connection`'s own `CREATE TABLE`), then confirm `ConnectionStore::open`
    /// against that same file still works afterward.
    #[test]
    fn opening_a_pre_installation_id_column_database_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pre-migration.sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "CREATE TABLE repo_connections (
                    id TEXT NOT NULL, tenant TEXT NOT NULL, repo_url TEXT NOT NULL, method TEXT NOT NULL,
                    public_key_openssh TEXT, encrypted_private_key_openssh TEXT, status TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')), PRIMARY KEY (tenant, id)
                )",
                [],
            )
            .unwrap();
            let keypair = test_keypair("acme", "old-repo");
            conn.execute(
                "INSERT INTO repo_connections (id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, status)
                 VALUES ('old-repo', 'acme', 'git@github.com:acme/old.git', 'ssh', ?1, ?2, 'pending')",
                rusqlite::params![keypair.public_key_openssh, keypair.encrypted_private_key_openssh],
            )
            .unwrap();
        }

        let store = ConnectionStore::open(&path).unwrap();
        let pre_existing = store.get_connection("acme", "old-repo").unwrap().unwrap();
        assert_eq!(pre_existing.installation_id, None, "a pre-migration row must read back with a null installation_id, not error");

        let new_keypair = test_keypair("acme", "new-repo");
        let created = store.create_ssh_connection("acme", "new-repo", "git@github.com:acme/new.git", &new_keypair).unwrap();
        assert_eq!(created.installation_id, None);
    }

    #[test]
    fn create_then_get_round_trips() {
        let store = test_store();
        let keypair = test_keypair("acme", "repo-1");
        let created = store.create_ssh_connection("acme", "repo-1", "git@github.com:acme/widgets.git", &keypair).unwrap();
        assert_eq!(created.status, ConnectionStatus::Pending);

        let fetched = store.get_connection("acme", "repo-1").unwrap().unwrap();
        assert_eq!(fetched.repo_url, "git@github.com:acme/widgets.git");
        assert_eq!(fetched.method, ConnectionMethod::Ssh);
        assert_eq!(fetched.public_key_openssh.as_deref(), Some(keypair.public_key_openssh.as_str()));
    }

    #[test]
    fn delete_all_for_tenant_removes_only_that_tenants_connections() {
        let store = test_store();
        let keypair_a1 = test_keypair("acme", "repo-1");
        let keypair_a2 = test_keypair("acme", "repo-2");
        let keypair_b = test_keypair("globex", "repo-1");
        store.create_ssh_connection("acme", "repo-1", "git@github.com:acme/widgets.git", &keypair_a1).unwrap();
        store.create_ssh_connection("acme", "repo-2", "git@github.com:acme/gizmos.git", &keypair_a2).unwrap();
        store.create_ssh_connection("globex", "repo-1", "git@github.com:globex/gadgets.git", &keypair_b).unwrap();

        let deleted = store.delete_all_for_tenant("acme").unwrap();
        assert_eq!(deleted, 2);
        assert!(store.list_connections("acme").unwrap().is_empty());
        assert_eq!(store.list_connections("globex").unwrap().len(), 1, "a different tenant's connections must survive");
    }

    #[test]
    fn one_tenant_never_sees_another_tenants_connections() {
        let store = test_store();
        let keypair_a = test_keypair("acme", "repo-1");
        let keypair_b = test_keypair("globex", "repo-1");
        store.create_ssh_connection("acme", "repo-1", "git@github.com:acme/widgets.git", &keypair_a).unwrap();
        store.create_ssh_connection("globex", "repo-1", "git@github.com:globex/gadgets.git", &keypair_b).unwrap();

        let acme_conns = store.list_connections("acme").unwrap();
        assert_eq!(acme_conns.len(), 1);
        assert_eq!(acme_conns[0].repo_url, "git@github.com:acme/widgets.git");

        // Same connection id ("repo-1") used by both tenants deliberately —
        // proves the lookup is scoped by (tenant, id), not id alone.
        assert!(store.get_connection("globex", "repo-1").unwrap().is_some());
        let globex_view_of_acme = store.get_connection("globex", "repo-1").unwrap().unwrap();
        assert_eq!(globex_view_of_acme.repo_url, "git@github.com:globex/gadgets.git");
    }

    #[test]
    fn create_discovered_connection_starts_pending_with_no_auth_material() {
        let store = test_store();
        let created = store.create_discovered_connection("acme", "repo-1", "git@github.com:acme/widgets.git", None).unwrap();
        assert_eq!(created.method, ConnectionMethod::Discovered);
        assert_eq!(created.status, ConnectionStatus::Pending);
        assert_eq!(created.public_key_openssh, None);
        assert_eq!(created.encrypted_private_key_openssh, None);
        assert_eq!(created.installation_id, None);

        let fetched = store.get_connection("acme", "repo-1").unwrap().unwrap();
        assert_eq!(fetched.repo_url, "git@github.com:acme/widgets.git");
    }

    #[test]
    fn is_local_only_url_distinguishes_the_synthetic_prefix_from_a_real_remote() {
        assert!(is_local_only_url("local:deadbeef"));
        assert!(!is_local_only_url("git@github.com:acme/widgets.git"));
        assert!(!is_local_only_url("https://github.com/acme/widgets.git"));
    }

    #[test]
    fn new_local_repo_id_is_non_empty_and_differs_across_calls() {
        let a = new_local_repo_id();
        let b = new_local_repo_id();
        assert!(!a.is_empty());
        assert_ne!(a, b);
    }

    #[test]
    fn set_status_transitions_pending_to_active() {
        let store = test_store();
        let keypair = test_keypair("acme", "repo-1");
        store.create_ssh_connection("acme", "repo-1", "url", &keypair).unwrap();

        store.set_status("acme", "repo-1", ConnectionStatus::Active).unwrap();
        let fetched = store.get_connection("acme", "repo-1").unwrap().unwrap();
        assert_eq!(fetched.status, ConnectionStatus::Active);
    }

    #[test]
    fn set_status_rejects_updating_a_nonexistent_connection() {
        let store = test_store();
        assert!(store.set_status("acme", "does-not-exist", ConnectionStatus::Active).is_err());
    }

    #[test]
    fn failed_status_round_trips_its_reason() {
        let store = test_store();
        let keypair = test_keypair("acme", "repo-1");
        store.create_ssh_connection("acme", "repo-1", "url", &keypair).unwrap();
        store.set_status("acme", "repo-1", ConnectionStatus::Failed("connection refused".into())).unwrap();

        let fetched = store.get_connection("acme", "repo-1").unwrap().unwrap();
        assert_eq!(fetched.status, ConnectionStatus::Failed("connection refused".into()));
    }

    #[test]
    fn delete_connection_removes_exactly_the_targeted_row() {
        let store = test_store();
        let keypair_a1 = test_keypair("acme", "repo-1");
        let keypair_a2 = test_keypair("acme", "repo-2");
        store.create_ssh_connection("acme", "repo-1", "git@github.com:acme/widgets.git", &keypair_a1).unwrap();
        store.create_ssh_connection("acme", "repo-2", "git@github.com:acme/gizmos.git", &keypair_a2).unwrap();

        let deleted = store.delete_connection("acme", "repo-1").unwrap();
        assert!(deleted);
        assert!(store.get_connection("acme", "repo-1").unwrap().is_none());
        assert!(store.get_connection("acme", "repo-2").unwrap().is_some(), "deleting repo-1 must not touch repo-2");
    }

    #[test]
    fn delete_connection_returns_false_for_a_connection_that_never_existed() {
        let store = test_store();
        assert!(!store.delete_connection("acme", "does-not-exist").unwrap());
    }

    #[test]
    fn attach_ssh_remote_succeeds_against_a_true_local_only_connection() {
        let store = test_store();
        store.create_discovered_connection("acme", "job-hunter", "local:job-hunter", None).unwrap();
        let keypair = test_keypair("acme", "job-hunter");

        let updated = store.attach_ssh_remote("acme", "job-hunter", "git@github.com:acme/job-hunter.git", &keypair).unwrap();
        assert_eq!(updated.id, "job-hunter");
        assert_eq!(updated.method, ConnectionMethod::Ssh);
        assert_eq!(updated.repo_url, "git@github.com:acme/job-hunter.git");
        assert_eq!(updated.status, ConnectionStatus::Pending);
    }

    #[test]
    fn attach_ssh_remote_succeeds_against_a_real_but_unauthenticated_discovered_connection() {
        let store = test_store();
        store.create_discovered_connection("acme", "acme--widgets", "git@github.com:acme/widgets.git", None).unwrap();
        let keypair = test_keypair("acme", "acme--widgets");

        // Same repo_url the Discovered row already had -- completing it, not repointing it.
        let updated = store.attach_ssh_remote("acme", "acme--widgets", "git@github.com:acme/widgets.git", &keypair).unwrap();
        assert_eq!(updated.method, ConnectionMethod::Ssh);
    }

    #[test]
    fn attach_ssh_remote_rejects_an_already_connected_repo() {
        let store = test_store();
        let keypair = test_keypair("acme", "job-hunter");
        store.create_ssh_connection("acme", "job-hunter", "git@github.com:acme/job-hunter.git", &keypair).unwrap();

        let err = store.attach_ssh_remote("acme", "job-hunter", "git@github.com:acme/job-hunter.git", &keypair).unwrap_err();
        assert!(err.to_string().contains("already Ssh"), "{err}");
    }

    #[test]
    fn attach_ssh_remote_rejects_repointing_a_discovered_connection_to_a_different_repo() {
        let store = test_store();
        store.create_discovered_connection("acme", "acme--widgets", "git@github.com:acme/widgets.git", None).unwrap();
        let keypair = test_keypair("acme", "acme--widgets");

        let err = store.attach_ssh_remote("acme", "acme--widgets", "git@github.com:acme/gizmos.git", &keypair).unwrap_err();
        assert!(err.to_string().contains("attaching a different remote isn't supported"), "{err}");
    }

    #[test]
    fn attach_ssh_remote_rejects_a_repo_url_already_used_by_a_different_connection() {
        let store = test_store();
        let keypair_a = test_keypair("acme", "repo-a");
        store.create_ssh_connection("acme", "repo-a", "git@github.com:acme/widgets.git", &keypair_a).unwrap();
        store.create_discovered_connection("acme", "repo-b", "local:repo-b", None).unwrap();
        let keypair_b = test_keypair("acme", "repo-b");

        let err = store.attach_ssh_remote("acme", "repo-b", "git@github.com:acme/widgets.git", &keypair_b).unwrap_err();
        assert!(err.to_string().contains("already connected as \"repo-a\""), "{err}");
    }

    #[test]
    fn attach_ssh_remote_normalizes_an_ssh_config_alias_against_the_canonical_host() {
        let store = test_store();
        let keypair_a = test_keypair("acme", "repo-a");
        store.create_ssh_connection("acme", "repo-a", "git@github.com:acme/widgets.git", &keypair_a).unwrap();
        store.create_discovered_connection("acme", "repo-b", "local:repo-b", None).unwrap();
        let keypair_b = test_keypair("acme", "repo-b");

        // Same repo via a custom SSH config host alias -- must still be
        // recognized as a duplicate of repo-a, per the recorded gotcha
        // about exact-string remote matching breaking on SSH aliases.
        let err = store.attach_ssh_remote("acme", "repo-b", "git@github-personal:acme/widgets.git", &keypair_b).unwrap_err();
        assert!(err.to_string().contains("already connected as \"repo-a\""), "{err}");
    }

    #[test]
    fn attach_github_app_remote_preserves_id_and_turns_discovered_into_active() {
        let store = test_store();
        store.create_discovered_connection("acme", "job-hunter", "local:job-hunter", None).unwrap();

        let updated = store.attach_github_app_remote("acme", "job-hunter", "https://github.com/acme/job-hunter.git", "install-123").unwrap();
        assert_eq!(updated.id, "job-hunter");
        assert_eq!(updated.method, ConnectionMethod::GitHubApp);
        assert_eq!(updated.status, ConnectionStatus::Active);
        assert_eq!(updated.installation_id, Some("install-123".to_string()));
    }

    #[test]
    fn attach_methods_reject_a_nonexistent_connection() {
        let store = test_store();
        let keypair = test_keypair("acme", "does-not-exist");
        let err = store.attach_ssh_remote("acme", "does-not-exist", "git@github.com:acme/widgets.git", &keypair).unwrap_err();
        assert!(err.to_string().contains("no connection"), "{err}");
    }

    #[test]
    fn find_discovered_local_only_connections_by_name_matches_case_insensitively() {
        let store = test_store();
        store.create_discovered_connection("acme", "job-hunter-uuid1", "local:job-hunter-uuid1", Some("job-hunter")).unwrap();

        let matches = store.find_discovered_local_only_connections_by_name("acme", "Job-Hunter").unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "job-hunter-uuid1");
    }

    #[test]
    fn find_discovered_local_only_connections_by_name_returns_every_match_not_just_one() {
        let store = test_store();
        store.create_discovered_connection("acme", "job-hunter-uuid1", "local:job-hunter-uuid1", Some("job-hunter")).unwrap();
        store.create_discovered_connection("acme", "job-hunter-uuid2", "local:job-hunter-uuid2", Some("job-hunter")).unwrap();

        let matches = store.find_discovered_local_only_connections_by_name("acme", "job-hunter").unwrap();
        assert_eq!(matches.len(), 2, "ambiguous name matches must all be surfaced, never silently picked between");
    }

    #[test]
    fn find_discovered_local_only_connections_by_name_ignores_a_real_url_discovered_row() {
        let store = test_store();
        // A real-but-unauthenticated Discovered row (register_repo's
        // repo_url path) must not be treated as a "local-only" match even
        // if its name happens to coincide -- it already has its own real
        // repo_url and is subject to the normal dedup guard instead.
        store.create_discovered_connection("acme", "acme--job-hunter", "https://github.com/acme/job-hunter.git", Some("job-hunter")).unwrap();

        let matches = store.find_discovered_local_only_connections_by_name("acme", "job-hunter").unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn find_discovered_local_only_connections_by_name_ignores_a_row_with_no_name() {
        let store = test_store();
        store.create_discovered_connection("acme", "old-stub", "local:old-stub", None).unwrap();

        let matches = store.find_discovered_local_only_connections_by_name("acme", "old-stub").unwrap();
        assert!(matches.is_empty(), "a pre-migration row with no stored name must never match, not fall back to matching on its id");
    }

    #[test]
    fn find_discovered_local_only_connections_by_name_is_tenant_scoped() {
        let store = test_store();
        store.create_discovered_connection("acme", "job-hunter-uuid1", "local:job-hunter-uuid1", Some("job-hunter")).unwrap();

        let matches = store.find_discovered_local_only_connections_by_name("globex", "job-hunter").unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn find_connection_by_repo_url_finds_a_connection_regardless_of_method() {
        let store = test_store();
        let keypair = test_keypair("acme", "repo-a");
        store.create_ssh_connection("acme", "repo-a", "git@github.com:acme/widgets.git", &keypair).unwrap();

        let found = store.find_connection_by_repo_url("acme", "https://github.com/acme/widgets.git").unwrap();
        assert_eq!(found.unwrap().id, "repo-a", "must match via normalize_repo_path across different URL shapes for the same repo");
    }

    #[test]
    fn find_connection_by_repo_url_returns_none_for_no_match() {
        let store = test_store();
        assert!(store.find_connection_by_repo_url("acme", "https://github.com/acme/nothing-here.git").unwrap().is_none());
    }
}
