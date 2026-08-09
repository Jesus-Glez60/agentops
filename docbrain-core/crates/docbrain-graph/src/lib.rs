//! Library / doc_snapshot / changelog_version / doc_node store — Docbrain-1/2
//! (version-aware "molecules") + Docbrain-4 (private libraries, access
//! control). SQLite-backed for the light tier, same architectural pattern as
//! `agentops-graph`'s `SqliteGraphStore`.
//!
//! **Tenant isolation is enforced at the type level, not by convention**: every
//! read/write method requires a `&TenantContext` as its first argument — there
//! is no method that queries `doc_nodes`/`libraries` without one, so "forgot to
//! scope the query" is impossible to write, not just discouraged. This is the
//! plan's §Security requirement ("the trait signature itself requires a tenant
//! context to construct a query, making the unscoped case a compile error, not
//! a runtime bug") applied concretely.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Who is asking. `Public` sees only public libraries; `Org` sees public
/// libraries plus whatever's scoped `private:<its own id>` — never another
/// org's private libraries, regardless of what the caller requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantContext {
    Public,
    Org(String),
}

impl TenantContext {
    pub fn public() -> Self {
        TenantContext::Public
    }

    pub fn org(id: impl Into<String>) -> Self {
        TenantContext::Org(id.into())
    }

    /// The exact `visibility` values this tenant is allowed to see.
    fn visible_scopes(&self) -> Vec<String> {
        match self {
            TenantContext::Public => vec!["public".to_string()],
            TenantContext::Org(id) => vec!["public".to_string(), format!("private:{id}")],
        }
    }

    /// A stable string key identifying exactly this tenant (not what it can
    /// *see*, unlike `visible_scopes` — this is who it *is*), used to scope
    /// `jobs` rows to their owner. Two different orgs must never share a
    /// job's status even if both happen to have started a job for the same
    /// library slug.
    fn key(&self) -> String {
        match self {
            TenantContext::Public => "public".to_string(),
            TenantContext::Org(id) => format!("org:{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Private(String),
}

impl Visibility {
    fn as_db_string(&self) -> String {
        match self {
            Visibility::Public => "public".to_string(),
            Visibility::Private(org) => format!("private:{org}"),
        }
    }

    fn from_db_string(s: &str) -> Self {
        match s.strip_prefix("private:") {
            Some(org) => Visibility::Private(org.to_string()),
            None => Visibility::Public,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub github_repo: Option<String>,
    pub docs_url: Option<String>,
    pub visibility: Visibility,
    /// How many `doc_snapshots` rows this library has -- one per
    /// scrape/ingest call, not one per version (a library can be
    /// re-scraped at the same version). Matches the field the user's
    /// existing production docbrain server's `list_libraries_tool` already
    /// returns.
    pub doc_snapshots: i64,
    pub changelog_versions: i64,
    /// Distinct versions with at least one doc_snapshot, ascending.
    pub versions: Vec<String>,
    pub total_nodes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocNode {
    pub id: i64,
    pub library_id: i64,
    pub version: String,
    pub topic: String,
    pub content: String,
}

/// A background ingestion job (`scrape_library`/`sync_changelogs` run in the
/// background — see `docbrain-mcp`'s `tool_scrape_library`/`tool_sync_changelogs`).
/// Scoped to the tenant that started it via `tenant_key`, not via
/// `visibility`/`visible_scopes` (a job isn't content with a
/// public/private-to-an-org visibility level — it's a private status record
/// belonging to whoever kicked it off, full stop).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub job_type: String,
    pub slug: String,
    pub status: JobStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
}

impl JobStatus {
    fn as_db_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
        }
    }

    fn from_db_str(s: &str) -> Self {
        match s {
            "succeeded" => JobStatus::Succeeded,
            "failed" => JobStatus::Failed,
            _ => JobStatus::Running,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub id: i64,
    pub library_id: i64,
    pub from_version: String,
    pub to_version: String,
    pub entry: String,
    pub breaking: bool,
}

pub struct DocbrainStore {
    conn: Connection,
}

impl DocbrainStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening docbrain store at {}", path.display()))?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    fn migrate(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS libraries (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                slug        TEXT NOT NULL UNIQUE,
                name        TEXT NOT NULL,
                github_repo TEXT,
                docs_url    TEXT,
                visibility  TEXT NOT NULL DEFAULT 'public'
            );

            CREATE TABLE IF NOT EXISTS doc_snapshots (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                library_id  INTEGER NOT NULL REFERENCES libraries(id),
                version     TEXT NOT NULL,
                scraped_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS changelog_versions (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                library_id   INTEGER NOT NULL REFERENCES libraries(id),
                from_version TEXT NOT NULL,
                to_version   TEXT NOT NULL,
                entry        TEXT NOT NULL,
                breaking     INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS doc_nodes (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                library_id  INTEGER NOT NULL REFERENCES libraries(id),
                version     TEXT NOT NULL,
                topic       TEXT NOT NULL,
                content     TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_doc_nodes_lib_ver ON doc_nodes(library_id, version);

            CREATE TABLE IF NOT EXISTS jobs (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                tenant_key  TEXT NOT NULL,
                job_type    TEXT NOT NULL,
                slug        TEXT NOT NULL,
                status      TEXT NOT NULL DEFAULT 'running',
                message     TEXT,
                created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            ",
        )?;
        Ok(())
    }

    /// Starts a job record in `running` state and returns its id. Called
    /// synchronously, right before spawning the background thread that does
    /// the actual work — so a caller polling `get_job` immediately after
    /// starting a background scrape always finds a real row, never "no such
    /// job" due to a race with the spawned thread not having inserted yet.
    pub fn create_job(&self, tenant: &TenantContext, job_type: &str, slug: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO jobs (tenant_key, job_type, slug, status) VALUES (?1, ?2, ?3, 'running')",
            rusqlite::params![tenant.key(), job_type, slug],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Marks a job done, successfully or not. Takes a plain `i64` id rather
    /// than a tenant-checked lookup — this is called from inside the
    /// background thread that owns the job, which already knows its own id;
    /// tenant scoping is enforced on the *read* side (`get_job`) instead.
    pub fn complete_job(&self, id: i64, status: JobStatus, message: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs SET status = ?1, message = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
            rusqlite::params![status.as_db_str(), message, id],
        )?;
        Ok(())
    }

    /// Looks up a job by id, but only if it belongs to `tenant` — one org
    /// polling another org's job id (even a guessed/incremented one) gets
    /// `Ok(None)`, indistinguishable from "no such job," same information-
    /// hiding posture as `get_library` for private libraries.
    pub fn get_job(&self, tenant: &TenantContext, id: i64) -> Result<Option<Job>> {
        let mut stmt = self.conn.prepare("SELECT * FROM jobs WHERE id = ?1 AND tenant_key = ?2")?;
        let mut rows = stmt.query_map(rusqlite::params![id, tenant.key()], |row| {
            let status_str: String = row.get("status")?;
            Ok(Job {
                id: row.get("id")?,
                job_type: row.get("job_type")?,
                slug: row.get("slug")?,
                status: JobStatus::from_db_str(&status_str),
                message: row.get("message")?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Aggregate stats for one library -- `doc_snapshots`/`changelog_versions`
    /// counts, distinct `versions`, and total `doc_nodes` count. Three plain
    /// `COUNT(*)` queries plus one `DISTINCT` query per library; fine at this
    /// scale (a handful of libraries per tenant, not thousands) and mirrors
    /// exactly what the user's real production docbrain server's
    /// `list_libraries_tool` already returns (verified live against it) --
    /// this isn't a new metric invented for this UI, it's closing a gap
    /// versus a system already proven to want these fields.
    fn stats_for(&self, library_id: i64) -> rusqlite::Result<(i64, i64, Vec<String>, i64)> {
        let doc_snapshots: i64 = self.conn.query_row("SELECT COUNT(*) FROM doc_snapshots WHERE library_id = ?1", [library_id], |r| r.get(0))?;
        let changelog_versions: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM changelog_versions WHERE library_id = ?1", [library_id], |r| r.get(0))?;
        let total_nodes: i64 = self.conn.query_row("SELECT COUNT(*) FROM doc_nodes WHERE library_id = ?1", [library_id], |r| r.get(0))?;
        let mut stmt = self.conn.prepare("SELECT DISTINCT version FROM doc_snapshots WHERE library_id = ?1 ORDER BY version")?;
        let versions = stmt.query_map([library_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((doc_snapshots, changelog_versions, versions, total_nodes))
    }

    fn row_to_library(&self, row: &rusqlite::Row) -> rusqlite::Result<Library> {
        let visibility_str: String = row.get("visibility")?;
        let id: i64 = row.get("id")?;
        let (doc_snapshots, changelog_versions, versions, total_nodes) = self.stats_for(id)?;
        Ok(Library {
            id,
            slug: row.get("slug")?,
            name: row.get("name")?,
            github_repo: row.get("github_repo")?,
            docs_url: row.get("docs_url")?,
            visibility: Visibility::from_db_string(&visibility_str),
            doc_snapshots,
            changelog_versions,
            versions,
            total_nodes,
        })
    }

    /// Registers a new library. A tenant may only create a library `Public` or
    /// `Private` scoped to *its own* org — attempting to create
    /// `Private("other-org")` while authenticated as a different org fails.
    pub fn add_library(
        &self,
        tenant: &TenantContext,
        slug: &str,
        name: &str,
        github_repo: Option<&str>,
        docs_url: Option<&str>,
        visibility: Visibility,
    ) -> Result<i64> {
        if let Visibility::Private(owner) = &visibility {
            match tenant {
                TenantContext::Org(id) if id == owner => {}
                _ => anyhow::bail!("cannot create a library private to '{owner}' while authenticated as {tenant:?}"),
            }
        }

        self.conn.execute(
            "INSERT INTO libraries (slug, name, github_repo, docs_url, visibility) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![slug, name, github_repo, docs_url, visibility.as_db_string()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Looks up a library by slug, scoped to what `tenant` is allowed to see.
    /// A private library belonging to a different org is indistinguishable
    /// from "doesn't exist" here — never returned, never leaked via an error
    /// message either.
    pub fn get_library(&self, tenant: &TenantContext, slug: &str) -> Result<Option<Library>> {
        let scopes = tenant.visible_scopes();
        let placeholders = scopes.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT * FROM libraries WHERE slug = ? AND visibility IN ({placeholders})");

        let mut stmt = self.conn.prepare(&sql)?;
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&slug];
        for s in &scopes {
            params.push(s);
        }
        let mut rows = stmt.query_map(params.as_slice(), |row| self.row_to_library(row))?;
        Ok(rows.next().transpose()?)
    }

    /// Lists every library visible to `tenant` — public libraries plus its own
    /// private ones, never another org's.
    pub fn list_libraries(&self, tenant: &TenantContext) -> Result<Vec<Library>> {
        let scopes = tenant.visible_scopes();
        let placeholders = scopes.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT * FROM libraries WHERE visibility IN ({placeholders}) ORDER BY slug");

        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = scopes.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |row| self.row_to_library(row))?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    /// Resolves `library_id` only if `tenant` is allowed to see it — every
    /// write below routes through this so a caller can't write doc nodes onto
    /// a library it can't even read.
    fn authorized_library_id(&self, tenant: &TenantContext, slug: &str) -> Result<i64> {
        self.get_library(tenant, slug)?
            .map(|l| l.id)
            .ok_or_else(|| anyhow::anyhow!("no library '{slug}' visible to {tenant:?}"))
    }

    pub fn add_doc_snapshot(&self, tenant: &TenantContext, slug: &str, version: &str) -> Result<i64> {
        let library_id = self.authorized_library_id(tenant, slug)?;
        self.conn.execute(
            "INSERT INTO doc_snapshots (library_id, version) VALUES (?1, ?2)",
            rusqlite::params![library_id, version],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn add_doc_node(&self, tenant: &TenantContext, slug: &str, version: &str, topic: &str, content: &str) -> Result<i64> {
        let library_id = self.authorized_library_id(tenant, slug)?;
        self.conn.execute(
            "INSERT INTO doc_nodes (library_id, version, topic, content) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![library_id, version, topic, content],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn add_changelog_entry(
        &self,
        tenant: &TenantContext,
        slug: &str,
        from_version: &str,
        to_version: &str,
        entry: &str,
        breaking: bool,
    ) -> Result<i64> {
        let library_id = self.authorized_library_id(tenant, slug)?;
        self.conn.execute(
            "INSERT INTO changelog_versions (library_id, from_version, to_version, entry, breaking) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![library_id, from_version, to_version, entry, breaking as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Docbrain-1/2: fine-grained version awareness. Returns doc nodes for the
    /// exact `version` requested (not the nearest major/minor) if the library
    /// is visible to `tenant`. Callers wanting "nearest available" resolution
    /// do that against `list_doc_versions` themselves — this method never
    /// silently substitutes a different version.
    pub fn get_doc_nodes(&self, tenant: &TenantContext, slug: &str, version: &str) -> Result<Vec<DocNode>> {
        let library_id = self.authorized_library_id(tenant, slug)?;
        let mut stmt = self.conn.prepare("SELECT * FROM doc_nodes WHERE library_id = ?1 AND version = ?2")?;
        let rows = stmt.query_map(rusqlite::params![library_id, version], |row| {
            Ok(DocNode {
                id: row.get("id")?,
                library_id: row.get("library_id")?,
                version: row.get("version")?,
                topic: row.get("topic")?,
                content: row.get("content")?,
            })
        })?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    /// Every doc node across every version of `slug` — unlike `get_doc_nodes`
    /// (Docbrain-2's exact-version guarantee, for retrieval), this is for
    /// bulk operations like semantic indexing where "all content this
    /// library has, regardless of version" is exactly what's wanted.
    pub fn all_doc_nodes(&self, tenant: &TenantContext, slug: &str) -> Result<Vec<DocNode>> {
        let library_id = self.authorized_library_id(tenant, slug)?;
        let mut stmt = self.conn.prepare("SELECT * FROM doc_nodes WHERE library_id = ?1")?;
        let rows = stmt.query_map([library_id], |row| {
            Ok(DocNode {
                id: row.get("id")?,
                library_id: row.get("library_id")?,
                version: row.get("version")?,
                topic: row.get("topic")?,
                content: row.get("content")?,
            })
        })?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    pub fn list_doc_versions(&self, tenant: &TenantContext, slug: &str) -> Result<Vec<String>> {
        let library_id = self.authorized_library_id(tenant, slug)?;
        let mut stmt = self.conn.prepare("SELECT DISTINCT version FROM doc_snapshots WHERE library_id = ?1 ORDER BY version")?;
        let rows = stmt.query_map([library_id], |row| row.get::<_, String>(0))?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    pub fn get_changelog(&self, tenant: &TenantContext, slug: &str, from_version: &str, to_version: &str) -> Result<Vec<ChangelogEntry>> {
        let library_id = self.authorized_library_id(tenant, slug)?;
        let mut stmt = self.conn.prepare(
            "SELECT * FROM changelog_versions WHERE library_id = ?1 AND from_version = ?2 AND to_version = ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![library_id, from_version, to_version], |row| {
            Ok(ChangelogEntry {
                id: row.get("id")?,
                library_id: row.get("library_id")?,
                from_version: row.get("from_version")?,
                to_version: row.get("to_version")?,
                entry: row.get("entry")?,
                breaking: row.get::<_, i64>("breaking")? != 0,
            })
        })?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_library_visible_to_everyone() {
        let store = DocbrainStore::open_in_memory().unwrap();
        store.add_library(&TenantContext::public(), "react", "React", Some("facebook/react"), Some("https://react.dev"), Visibility::Public).unwrap();

        assert!(store.get_library(&TenantContext::public(), "react").unwrap().is_some());
        assert!(store.get_library(&TenantContext::org("acme"), "react").unwrap().is_some());
    }

    #[test]
    fn library_stats_reflect_real_snapshots_changelog_and_doc_node_counts() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let pub_ctx = TenantContext::public();
        store.add_library(&pub_ctx, "next", "Next.js", None, Some("https://nextjs.org/docs"), Visibility::Public).unwrap();

        // Two snapshots means two doc_snapshots rows even though one of the
        // versions gets scraped twice below (the count isn't "distinct
        // versions", it's "how many times has this been scraped") -- see
        // the field's own doc comment for that distinction.
        store.add_doc_snapshot(&pub_ctx, "next", "15.1.2").unwrap();
        store.add_doc_snapshot(&pub_ctx, "next", "15.1.3").unwrap();
        store.add_doc_snapshot(&pub_ctx, "next", "15.1.3").unwrap();
        store.add_doc_node(&pub_ctx, "next", "15.1.2", "routing", "content a").unwrap();
        store.add_doc_node(&pub_ctx, "next", "15.1.3", "routing", "content b").unwrap();
        store.add_doc_node(&pub_ctx, "next", "15.1.3", "caching", "content c").unwrap();
        store.add_changelog_entry(&pub_ctx, "next", "15.1.2", "15.1.3", "Fixed a routing regression.", true).unwrap();

        let lib = store.get_library(&pub_ctx, "next").unwrap().unwrap();
        assert_eq!(lib.doc_snapshots, 3);
        assert_eq!(lib.changelog_versions, 1);
        assert_eq!(lib.versions, vec!["15.1.2".to_string(), "15.1.3".to_string()]);
        assert_eq!(lib.total_nodes, 3);

        // list_libraries computes the same stats per row, not just get_library.
        let listed = store.list_libraries(&pub_ctx).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].total_nodes, 3);
    }

    #[test]
    fn a_library_with_no_scrapes_yet_reports_zeroed_stats_not_an_error() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let pub_ctx = TenantContext::public();
        store.add_library(&pub_ctx, "ghost", "Ghost", None, None, Visibility::Public).unwrap();

        let lib = store.get_library(&pub_ctx, "ghost").unwrap().unwrap();
        assert_eq!(lib.doc_snapshots, 0);
        assert_eq!(lib.changelog_versions, 0);
        assert!(lib.versions.is_empty());
        assert_eq!(lib.total_nodes, 0);
    }

    #[test]
    fn private_library_never_visible_to_a_different_org() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let acme = TenantContext::org("acme");
        store.add_library(&acme, "acme-internal-sdk", "Acme Internal SDK", None, None, Visibility::Private("acme".into())).unwrap();

        // Owning org sees it.
        assert!(store.get_library(&acme, "acme-internal-sdk").unwrap().is_some());

        // A different org does not — not even under list_libraries.
        let other = TenantContext::org("globex");
        assert!(store.get_library(&other, "acme-internal-sdk").unwrap().is_none());
        assert!(store.list_libraries(&other).unwrap().is_empty());

        // Nor does an unauthenticated (public) caller.
        assert!(store.get_library(&TenantContext::public(), "acme-internal-sdk").unwrap().is_none());
    }

    #[test]
    fn cannot_create_a_library_private_to_another_org() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let acme = TenantContext::org("acme");
        let result = store.add_library(&acme, "sneaky", "Sneaky", None, None, Visibility::Private("globex".into()));
        assert!(result.is_err(), "acme must not be able to create a library scoped private to globex");
    }

    #[test]
    fn writes_are_blocked_against_a_library_the_tenant_cannot_see() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let acme = TenantContext::org("acme");
        store.add_library(&acme, "acme-internal-sdk", "Acme Internal SDK", None, None, Visibility::Private("acme".into())).unwrap();

        let globex = TenantContext::org("globex");
        let result = store.add_doc_node(&globex, "acme-internal-sdk", "1.0", "overview", "hijacked content");
        assert!(result.is_err(), "globex must not be able to write doc nodes onto a library it can't see");
    }

    #[test]
    fn version_exact_lookup_never_substitutes_a_different_version() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let pub_ctx = TenantContext::public();
        store.add_library(&pub_ctx, "next", "Next.js", Some("vercel/next.js"), Some("https://nextjs.org/docs"), Visibility::Public).unwrap();
        store.add_doc_node(&pub_ctx, "next", "15.1.2", "routing", "15.1.2 routing docs").unwrap();
        store.add_doc_node(&pub_ctx, "next", "15.1.3", "routing", "15.1.3 routing docs (patched)").unwrap();

        let v2 = store.get_doc_nodes(&pub_ctx, "next", "15.1.2").unwrap();
        assert_eq!(v2.len(), 1);
        assert_eq!(v2[0].content, "15.1.2 routing docs");

        let v3 = store.get_doc_nodes(&pub_ctx, "next", "15.1.3").unwrap();
        assert_eq!(v3.len(), 1);
        assert_eq!(v3[0].content, "15.1.3 routing docs (patched)");
    }

    #[test]
    fn all_doc_nodes_spans_every_version() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let pub_ctx = TenantContext::public();
        store.add_library(&pub_ctx, "next", "Next.js", None, Some("https://nextjs.org/docs"), Visibility::Public).unwrap();
        store.add_doc_node(&pub_ctx, "next", "15.1.2", "routing", "15.1.2 routing docs").unwrap();
        store.add_doc_node(&pub_ctx, "next", "15.1.3", "routing", "15.1.3 routing docs (patched)").unwrap();

        let all = store.all_doc_nodes(&pub_ctx, "next").unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn job_lifecycle_running_then_succeeded() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let tenant = TenantContext::public();
        let id = store.create_job(&tenant, "scrape_library", "next").unwrap();

        let job = store.get_job(&tenant, id).unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.slug, "next");

        store.complete_job(id, JobStatus::Succeeded, "Scraped 3 sections.").unwrap();
        let job = store.get_job(&tenant, id).unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.message.as_deref(), Some("Scraped 3 sections."));
    }

    #[test]
    fn a_job_is_never_visible_to_a_different_tenant() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let acme = TenantContext::org("acme");
        let id = store.create_job(&acme, "scrape_library", "acme-internal-sdk").unwrap();

        let globex = TenantContext::org("globex");
        assert!(store.get_job(&globex, id).unwrap().is_none());
        assert!(store.get_job(&TenantContext::public(), id).unwrap().is_none());
        assert!(store.get_job(&acme, id).unwrap().is_some());
    }

    #[test]
    fn changelog_between_exact_versions() {
        let store = DocbrainStore::open_in_memory().unwrap();
        let pub_ctx = TenantContext::public();
        store.add_library(&pub_ctx, "next", "Next.js", None, None, Visibility::Public).unwrap();
        store.add_changelog_entry(&pub_ctx, "next", "15.1.2", "15.1.3", "Fixed a routing regression.", true).unwrap();

        let entries = store.get_changelog(&pub_ctx, "next", "15.1.2", "15.1.3").unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].breaking);
    }
}
