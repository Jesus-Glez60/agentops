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
                status                      TEXT NOT NULL,
                created_at                  TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (tenant, id)
            )",
            [],
        )
        .context("creating repo_connections table")?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_repo_connections_tenant ON repo_connections(tenant)", [])
            .context("creating tenant index")?;
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

    pub fn get_connection(&self, tenant: &str, id: &str) -> Result<Option<RepoConnection>> {
        self.conn
            .query_row(
                "SELECT id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, status, created_at
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
                "SELECT id, tenant, repo_url, method, public_key_openssh, encrypted_private_key_openssh, status, created_at
                 FROM repo_connections WHERE tenant = ?1 ORDER BY created_at DESC",
            )
            .context("preparing list query")?;
        let rows = stmt.query_map([tenant], row_to_connection).context("querying repo connections")?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("reading repo connection rows")
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
}

fn row_to_connection(row: &rusqlite::Row) -> rusqlite::Result<RepoConnection> {
    let method_str: String = row.get(3)?;
    let status_str: String = row.get(6)?;
    Ok(RepoConnection {
        id: row.get(0)?,
        tenant: row.get(1)?,
        repo_url: row.get(2)?,
        method: ConnectionMethod::from_str(&method_str).map_err(|e| rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.into()))?,
        public_key_openssh: row.get(4)?,
        encrypted_private_key_openssh: row.get(5)?,
        status: ConnectionStatus::from_db_string(&status_str),
        created_at: row.get(7)?,
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
