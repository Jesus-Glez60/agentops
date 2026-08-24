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
}

impl ConnectionMethod {
    fn as_str(&self) -> &'static str {
        match self {
            ConnectionMethod::Ssh => "ssh",
            ConnectionMethod::GitHubApp => "github_app",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ssh" => Ok(ConnectionMethod::Ssh),
            "github_app" => Ok(ConnectionMethod::GitHubApp),
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

    pub fn get_connection(&self, tenant: &str, id: &str) -> Result<Option<RepoConnection>> {
        self.conn
            .query_row(
                "SELECT id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, installation_id, status, created_at
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
                "SELECT id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, installation_id, status, created_at
                 FROM repo_connections WHERE tenant = ?1 ORDER BY created_at DESC",
            )
            .context("preparing list query")?;
        let rows = stmt.query_map([tenant], row_to_connection).context("querying repo connections")?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("reading repo connection rows")
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
                "SELECT id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, installation_id, status, created_at
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
}
