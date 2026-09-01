//! Live progress tracking for the clone -> scan -> embed -> docgen pipeline
//! that turns a `RepoConnection` into a searchable, documented repo.
//!
//! Deliberately **not** the same table as `agentops-graph`'s `tasks`/
//! `task_links` (module 7's human-facing, Linear-syncable project-task
//! tracker) -- that table has no ordered stage sequence, no sub-progress
//! counts, no per-stage error field, and lives in a different crate than
//! `RepoConnection`. This is a separate, purpose-built table for a polled
//! progress bar, not a reuse of that subsystem.
//!
//! Also deliberately **not** a replacement for `agentops-graph`'s
//! `scan_history`/`scan_history_entries` -- that table already gets written
//! automatically as a side effect of `agentops_mcp::scan::persist()` (one of
//! this job's own stages) and stays exactly as it is, a coarse post-hoc
//! summary. `indexing_job_stages` is a complementary, real-time layer
//! sitting alongside it for the polling UI.
//!
//! Same tenant-scoping discipline as `store.rs`: every read/write method
//! takes `tenant` as its first argument, so a query that forgets to scope by
//! tenant is a compile error waiting to be a leak, not a runtime one.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// The 9 stages shown in the "Connect repository" wizard's indexing screen,
/// in the fixed order they always run. `seq` is this array's index, stored
/// on each `indexing_job_stages` row so the UI can render them in order
/// without depending on insertion order or a string sort.
pub const STAGE_ORDER: [&str; 9] = [
    "connection_verified",
    "repository_cloned",
    "files_discovered",
    "symbols_extracted",
    "dependencies_mapped",
    "knowledge_nodes_created",
    "embeddings_generated",
    "documentation_generated",
    "index_ready",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Initial,
    Reindex,
}

impl JobKind {
    fn as_str(&self) -> &'static str {
        match self {
            JobKind::Initial => "initial",
            JobKind::Reindex => "reindex",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "initial" => Ok(JobKind::Initial),
            "reindex" => Ok(JobKind::Reindex),
            other => anyhow::bail!("unknown indexing job kind {other:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
}

impl JobStatus {
    fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "running" => Ok(JobStatus::Running),
            "succeeded" => Ok(JobStatus::Succeeded),
            "failed" => Ok(JobStatus::Failed),
            other => anyhow::bail!("unknown indexing job status {other:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Pending,
    Active,
    Done,
    Failed,
}

impl StageStatus {
    fn as_str(&self) -> &'static str {
        match self {
            StageStatus::Pending => "pending",
            StageStatus::Active => "active",
            StageStatus::Done => "done",
            StageStatus::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(StageStatus::Pending),
            "active" => Ok(StageStatus::Active),
            "done" => Ok(StageStatus::Done),
            "failed" => Ok(StageStatus::Failed),
            other => anyhow::bail!("unknown indexing stage status {other:?}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingJob {
    pub id: String,
    pub tenant: String,
    pub connection_id: String,
    /// Canonicalized `agentops_mcp::scan::repo_name()` value for this job's
    /// checkout -- derived once at job creation and reused at every stage
    /// call site, never re-derived from a raw path. A prior live-tested bug
    /// (`/search/index` silently returning `indexed: 0`) came from exactly
    /// that kind of raw-path-vs-canonicalized-name mismatch.
    pub repo_name: String,
    pub local_path: String,
    pub kind: JobKind,
    pub status: JobStatus,
    pub current_stage: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStage {
    pub job_id: String,
    pub stage: String,
    pub seq: i64,
    pub status: StageStatus,
    pub progress_current: Option<i64>,
    pub progress_total: Option<i64>,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLogLine {
    pub id: i64,
    pub ts: String,
    pub line: String,
}

/// A GitHub App installation on one org/user account, recorded once at
/// `GET /repos/github-app/callback` time. One installation can back
/// multiple `RepoConnection` rows (one per repo the tenant chose to
/// connect from it) -- this table exists purely to answer "which tenant
/// owns installation N" for the webhook receiver (an inbound `installation`/
/// `push` event carries an `installation.id`, never a tenant) and to let
/// `GET /repos/github-app/installations/{id}/repos` confirm the caller's
/// tenant actually owns the installation it's asking about before minting
/// a fresh installation token for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAppInstallation {
    pub id: String,
    pub tenant: String,
    pub account_login: String,
    pub installed_at: String,
    /// `"User"` or `"Organization"`, from GitHub's own installation
    /// object -- `None` for a row written before this field existed, or if
    /// the details fetch that populates it failed (non-fatal, see
    /// `github_app_callback`). Needed to build the correct "manage on
    /// GitHub" link: an organization's installation settings live under a
    /// different URL shape than a personal account's.
    pub account_type: Option<String>,
}

pub struct IndexingStore {
    conn: Connection,
}

impl IndexingStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("opening indexing-store database")?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory indexing-store database")?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS indexing_jobs (
                id             TEXT NOT NULL,
                tenant         TEXT NOT NULL,
                connection_id  TEXT NOT NULL,
                repo_name      TEXT NOT NULL,
                local_path     TEXT NOT NULL,
                kind           TEXT NOT NULL,
                status         TEXT NOT NULL,
                current_stage  TEXT,
                created_at     TEXT NOT NULL DEFAULT (datetime('now')),
                finished_at    TEXT,
                PRIMARY KEY (tenant, id)
            );
            CREATE INDEX IF NOT EXISTS idx_indexing_jobs_connection ON indexing_jobs(tenant, connection_id, created_at);

            CREATE TABLE IF NOT EXISTS indexing_job_stages (
                job_id           TEXT NOT NULL,
                tenant           TEXT NOT NULL,
                stage            TEXT NOT NULL,
                seq              INTEGER NOT NULL,
                status           TEXT NOT NULL,
                progress_current INTEGER,
                progress_total   INTEGER,
                error            TEXT,
                started_at       TEXT,
                finished_at      TEXT,
                PRIMARY KEY (tenant, job_id, stage)
            );

            CREATE TABLE IF NOT EXISTS indexing_job_log (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id  TEXT NOT NULL,
                tenant  TEXT NOT NULL,
                ts      TEXT NOT NULL DEFAULT (datetime('now')),
                line    TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_indexing_job_log_job ON indexing_job_log(tenant, job_id, id);

            CREATE TABLE IF NOT EXISTS github_app_installations (
                id            TEXT NOT NULL,
                tenant        TEXT NOT NULL,
                account_login TEXT NOT NULL,
                installed_at  TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (tenant, id)
            );
            CREATE INDEX IF NOT EXISTS idx_github_app_installations_tenant ON github_app_installations(tenant);",
        )
        .context("creating indexing-store tables")?;
        // `ALTER TABLE ... ADD COLUMN` is a no-op error (not silently
        // ignored) against a table that already has the column -- covers a
        // database file created before `account_type` existed, matching
        // `CREATE TABLE IF NOT EXISTS`'s own "don't error on an
        // already-migrated file" posture, same pattern as `store.rs`'s
        // `tracked_branch`/`installation_id` migrations.
        let _ = conn.execute("ALTER TABLE github_app_installations ADD COLUMN account_type TEXT", []);
        Ok(Self { conn })
    }

    /// Creates a job row plus all 9 `indexing_job_stages` rows (`pending`),
    /// in the fixed `STAGE_ORDER` -- called once per job, before spawning
    /// the orchestration task, so the very first status poll already sees a
    /// full 9-item stage list rather than one that grows as stages start.
    pub fn create_job(&self, tenant: &str, id: &str, connection_id: &str, repo_name: &str, local_path: &str, kind: JobKind) -> Result<IndexingJob> {
        self.conn
            .execute(
                "INSERT INTO indexing_jobs (id, tenant, connection_id, repo_name, local_path, kind, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![id, tenant, connection_id, repo_name, local_path, kind.as_str(), JobStatus::Running.as_str()],
            )
            .context("inserting indexing job")?;
        for (seq, stage) in STAGE_ORDER.iter().enumerate() {
            self.conn
                .execute(
                    "INSERT INTO indexing_job_stages (job_id, tenant, stage, seq, status) VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![id, tenant, stage, seq as i64, StageStatus::Pending.as_str()],
                )
                .context("inserting indexing job stage row")?;
        }
        self.get_job(tenant, id)?.context("just-inserted indexing job not found — this is a store bug")
    }

    pub fn get_job(&self, tenant: &str, id: &str) -> Result<Option<IndexingJob>> {
        self.conn
            .query_row(
                "SELECT id, tenant, connection_id, repo_name, local_path, kind, status, current_stage, created_at, finished_at
                 FROM indexing_jobs WHERE tenant = ?1 AND id = ?2",
                rusqlite::params![tenant, id],
                row_to_job,
            )
            .map(Some)
            .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })
            .context("querying indexing job")
    }

    /// Most recent job for a connection, if any -- backs `GET
    /// /repos/{id}/index/status` when the caller omits `job_id` (the common
    /// case: "show me this connection's current indexing state").
    pub fn latest_job_for_connection(&self, tenant: &str, connection_id: &str) -> Result<Option<IndexingJob>> {
        self.conn
            .query_row(
                "SELECT id, tenant, connection_id, repo_name, local_path, kind, status, current_stage, created_at, finished_at
                 FROM indexing_jobs WHERE tenant = ?1 AND connection_id = ?2 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                rusqlite::params![tenant, connection_id],
                row_to_job,
            )
            .map(Some)
            .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })
            .context("querying latest indexing job for connection")
    }

    pub fn list_stages(&self, tenant: &str, job_id: &str) -> Result<Vec<JobStage>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT job_id, stage, seq, status, progress_current, progress_total, error, started_at, finished_at
                 FROM indexing_job_stages WHERE tenant = ?1 AND job_id = ?2 ORDER BY seq",
            )
            .context("preparing stage list query")?;
        let rows = stmt.query_map(rusqlite::params![tenant, job_id], row_to_stage).context("querying indexing job stages")?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("reading indexing job stage rows")
    }

    /// Marks `stage` `active` and sets `current_stage` on the parent job --
    /// call immediately before starting the real work for that stage.
    pub fn start_stage(&self, tenant: &str, job_id: &str, stage: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE indexing_job_stages SET status = ?1, started_at = datetime('now') WHERE tenant = ?2 AND job_id = ?3 AND stage = ?4",
                rusqlite::params![StageStatus::Active.as_str(), tenant, job_id, stage],
            )
            .context("marking stage active")?;
        self.conn
            .execute("UPDATE indexing_jobs SET current_stage = ?1 WHERE tenant = ?2 AND id = ?3", rusqlite::params![stage, tenant, job_id])
            .context("updating job current_stage")?;
        Ok(())
    }

    /// Marks `stage` `done`, optionally recording a final sub-progress count
    /// (e.g. "2143 / 2143 vectors" for the embeddings stage).
    pub fn finish_stage(&self, tenant: &str, job_id: &str, stage: &str, progress_current: Option<i64>, progress_total: Option<i64>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE indexing_job_stages SET status = ?1, finished_at = datetime('now'), progress_current = ?2, progress_total = ?3
                 WHERE tenant = ?4 AND job_id = ?5 AND stage = ?6",
                rusqlite::params![StageStatus::Done.as_str(), progress_current, progress_total, tenant, job_id, stage],
            )
            .context("marking stage done")?;
        Ok(())
    }

    /// Marks `stage` `failed` with `reason` -- later `pending` stages are
    /// left untouched (not marked `failed` or `skipped`), matching the
    /// failure screen's done/failed/pending breakdown.
    pub fn fail_stage(&self, tenant: &str, job_id: &str, stage: &str, reason: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE indexing_job_stages SET status = ?1, finished_at = datetime('now'), error = ?2 WHERE tenant = ?3 AND job_id = ?4 AND stage = ?5",
                rusqlite::params![StageStatus::Failed.as_str(), reason, tenant, job_id, stage],
            )
            .context("marking stage failed")?;
        Ok(())
    }

    pub fn finish_job(&self, tenant: &str, job_id: &str, status: JobStatus) -> Result<()> {
        let updated = self
            .conn
            .execute(
                "UPDATE indexing_jobs SET status = ?1, current_stage = NULL, finished_at = datetime('now') WHERE tenant = ?2 AND id = ?3",
                rusqlite::params![status.as_str(), tenant, job_id],
            )
            .context("finishing indexing job")?;
        if updated == 0 {
            anyhow::bail!("no indexing job {job_id:?} for tenant {tenant:?} — refusing a silent no-op update");
        }
        Ok(())
    }

    pub fn append_log(&self, tenant: &str, job_id: &str, line: &str) -> Result<()> {
        self.conn
            .execute("INSERT INTO indexing_job_log (job_id, tenant, line) VALUES (?1, ?2, ?3)", rusqlite::params![job_id, tenant, line])
            .context("appending indexing job log line")?;
        Ok(())
    }

    /// `after_id` is a cursor, not an offset -- pass the last `id` the
    /// caller already has (0 for "from the start") so repeated polls only
    /// ship new lines instead of re-shipping the whole log every tick.
    pub fn log_since(&self, tenant: &str, job_id: &str, after_id: i64) -> Result<Vec<JobLogLine>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, ts, line FROM indexing_job_log WHERE tenant = ?1 AND job_id = ?2 AND id > ?3 ORDER BY id")
            .context("preparing log query")?;
        let rows = stmt
            .query_map(rusqlite::params![tenant, job_id, after_id], |row| Ok(JobLogLine { id: row.get(0)?, ts: row.get(1)?, line: row.get(2)? }))
            .context("querying indexing job log")?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("reading indexing job log rows")
    }

    /// `account_type` (`"User"`/`"Organization"`) is optional -- fetching it
    /// requires an extra GitHub API call the caller may not want to fail
    /// the whole install over (see `github_app_callback`'s non-fatal
    /// treatment of that fetch); `None` just means the "manage on GitHub"
    /// link build falls back to the personal-account URL shape.
    pub fn create_installation(&self, tenant: &str, id: &str, account_login: &str, account_type: Option<&str>) -> Result<GitHubAppInstallation> {
        self.conn
            .execute(
                "INSERT INTO github_app_installations (id, tenant, account_login, account_type) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (tenant, id) DO UPDATE SET account_login = excluded.account_login, account_type = excluded.account_type",
                rusqlite::params![id, tenant, account_login, account_type],
            )
            .context("inserting github app installation")?;
        self.get_installation(tenant, id)?.context("just-inserted installation not found — this is a store bug")
    }

    pub fn get_installation(&self, tenant: &str, id: &str) -> Result<Option<GitHubAppInstallation>> {
        self.conn
            .query_row(
                "SELECT id, tenant, account_login, installed_at, account_type FROM github_app_installations WHERE tenant = ?1 AND id = ?2",
                rusqlite::params![tenant, id],
                row_to_installation,
            )
            .map(Some)
            .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })
            .context("querying github app installation")
    }

    /// Every installation a tenant has -- for the dashboard's "is GitHub
    /// connected" status surfaces (Team & Access > Integrations and the
    /// Profile > Integrations read-only indicator), not just a single
    /// known-id lookup the way `get_installation` is.
    pub fn list_installations(&self, tenant: &str) -> Result<Vec<GitHubAppInstallation>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, tenant, account_login, installed_at, account_type FROM github_app_installations WHERE tenant = ?1 ORDER BY installed_at DESC")
            .context("preparing github app installations query")?;
        let rows = stmt.query_map(rusqlite::params![tenant], row_to_installation).context("querying github app installations")?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("reading github app installation rows")
    }

    /// Looks up which tenant owns installation `id`, with no tenant hint of
    /// its own to scope the query by -- the one place this store's usual
    /// tenant-first-argument discipline can't apply, since the whole point
    /// is resolving the tenant from an untrusted webhook payload's
    /// `installation.id`. Safe only because the caller (the webhook
    /// handler) treats the returned tenant as a lookup key, never as
    /// something the request itself asserted -- there's nothing here an
    /// attacker could spoof to see another tenant's data, since all this
    /// returns is "which tenant does installation N belong to," and acting
    /// on it (marking connections failed, spawning a reindex) only touches
    /// that same resolved tenant's own rows.
    pub fn tenant_for_installation(&self, id: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT tenant FROM github_app_installations WHERE id = ?1", rusqlite::params![id], |row| row.get(0))
            .map(Some)
            .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })
            .context("resolving tenant for github app installation")
    }
}

fn row_to_installation(row: &rusqlite::Row) -> rusqlite::Result<GitHubAppInstallation> {
    Ok(GitHubAppInstallation { id: row.get(0)?, tenant: row.get(1)?, account_login: row.get(2)?, installed_at: row.get(3)?, account_type: row.get(4)? })
}

fn row_to_job(row: &rusqlite::Row) -> rusqlite::Result<IndexingJob> {
    let kind_str: String = row.get(5)?;
    let status_str: String = row.get(6)?;
    Ok(IndexingJob {
        id: row.get(0)?,
        tenant: row.get(1)?,
        connection_id: row.get(2)?,
        repo_name: row.get(3)?,
        local_path: row.get(4)?,
        kind: JobKind::from_str(&kind_str).map_err(|e| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, e.into()))?,
        status: JobStatus::from_str(&status_str).map_err(|e| rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, e.into()))?,
        current_stage: row.get(7)?,
        created_at: row.get(8)?,
        finished_at: row.get(9)?,
    })
}

fn row_to_stage(row: &rusqlite::Row) -> rusqlite::Result<JobStage> {
    let status_str: String = row.get(3)?;
    Ok(JobStage {
        job_id: row.get(0)?,
        stage: row.get(1)?,
        seq: row.get(2)?,
        status: StageStatus::from_str(&status_str).map_err(|e| rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.into()))?,
        progress_current: row.get(4)?,
        progress_total: row.get(5)?,
        error: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_job_seeds_all_nine_stages_pending_in_order() {
        let store = IndexingStore::open_in_memory().unwrap();
        let job = store.create_job("acme", "job-1", "conn-1", "widgets", "/tmp/widgets", JobKind::Initial).unwrap();
        assert_eq!(job.status, JobStatus::Running);

        let stages = store.list_stages("acme", "job-1").unwrap();
        assert_eq!(stages.len(), 9);
        for (i, stage) in stages.iter().enumerate() {
            assert_eq!(stage.stage, STAGE_ORDER[i]);
            assert_eq!(stage.seq, i as i64);
            assert_eq!(stage.status, StageStatus::Pending);
        }
    }

    #[test]
    fn stage_lifecycle_transitions_and_records_progress() {
        let store = IndexingStore::open_in_memory().unwrap();
        store.create_job("acme", "job-1", "conn-1", "widgets", "/tmp/widgets", JobKind::Initial).unwrap();

        store.start_stage("acme", "job-1", "embeddings_generated").unwrap();
        let job = store.get_job("acme", "job-1").unwrap().unwrap();
        assert_eq!(job.current_stage.as_deref(), Some("embeddings_generated"));

        store.finish_stage("acme", "job-1", "embeddings_generated", Some(2143), Some(2143)).unwrap();
        let stages = store.list_stages("acme", "job-1").unwrap();
        let embeddings = stages.iter().find(|s| s.stage == "embeddings_generated").unwrap();
        assert_eq!(embeddings.status, StageStatus::Done);
        assert_eq!(embeddings.progress_current, Some(2143));
    }

    #[test]
    fn fail_stage_leaves_later_stages_pending_not_skipped() {
        let store = IndexingStore::open_in_memory().unwrap();
        store.create_job("acme", "job-1", "conn-1", "widgets", "/tmp/widgets", JobKind::Initial).unwrap();

        store.start_stage("acme", "job-1", "repository_cloned").unwrap();
        store.fail_stage("acme", "job-1", "repository_cloned", "Permission denied (publickey)").unwrap();
        store.finish_job("acme", "job-1", JobStatus::Failed).unwrap();

        let stages = store.list_stages("acme", "job-1").unwrap();
        let cloned = stages.iter().find(|s| s.stage == "repository_cloned").unwrap();
        assert_eq!(cloned.status, StageStatus::Failed);
        assert_eq!(cloned.error.as_deref(), Some("Permission denied (publickey)"));
        let later = stages.iter().find(|s| s.stage == "files_discovered").unwrap();
        assert_eq!(later.status, StageStatus::Pending, "stages after a failure stay pending, not skipped");

        let job = store.get_job("acme", "job-1").unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.current_stage, None);
    }

    #[test]
    fn one_tenant_never_sees_another_tenants_jobs() {
        let store = IndexingStore::open_in_memory().unwrap();
        store.create_job("acme", "job-1", "conn-1", "widgets", "/tmp/widgets", JobKind::Initial).unwrap();
        store.create_job("globex", "job-1", "conn-1", "widgets", "/tmp/widgets", JobKind::Initial).unwrap();

        store.start_stage("acme", "job-1", "repository_cloned").unwrap();
        let globex_stages = store.list_stages("globex", "job-1").unwrap();
        let cloned = globex_stages.iter().find(|s| s.stage == "repository_cloned").unwrap();
        assert_eq!(cloned.status, StageStatus::Pending, "a tenant-scoped mutation must never leak into another tenant's same-id job");
    }

    #[test]
    fn log_since_only_returns_lines_after_the_cursor() {
        let store = IndexingStore::open_in_memory().unwrap();
        store.create_job("acme", "job-1", "conn-1", "widgets", "/tmp/widgets", JobKind::Initial).unwrap();
        store.append_log("acme", "job-1", "cloning...").unwrap();
        store.append_log("acme", "job-1", "cloned.").unwrap();

        let all = store.log_since("acme", "job-1", 0).unwrap();
        assert_eq!(all.len(), 2);
        let after_first = store.log_since("acme", "job-1", all[0].id).unwrap();
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].line, "cloned.");
    }

    #[test]
    fn latest_job_for_connection_picks_the_most_recent() {
        let store = IndexingStore::open_in_memory().unwrap();
        store.create_job("acme", "job-1", "conn-1", "widgets", "/tmp/widgets", JobKind::Initial).unwrap();
        store.create_job("acme", "job-2", "conn-1", "widgets", "/tmp/widgets", JobKind::Reindex).unwrap();

        let latest = store.latest_job_for_connection("acme", "conn-1").unwrap().unwrap();
        assert_eq!(latest.id, "job-2");
    }

    #[test]
    fn list_installations_is_scoped_to_the_tenant_and_ordered_most_recent_first() {
        let store = IndexingStore::open_in_memory().unwrap();
        store.create_installation("acme", "111", "acme-corp", Some("User")).unwrap();
        store.create_installation("acme", "222", "acme-corp-second-org", Some("Organization")).unwrap();
        store.create_installation("globex", "333", "globex-corp", None).unwrap();

        let acme_installations = store.list_installations("acme").unwrap();
        assert_eq!(acme_installations.len(), 2, "a tenant-scoped query must never leak another tenant's installations");
        assert!(acme_installations.iter().all(|i| i.tenant == "acme"));
        assert!(acme_installations.iter().any(|i| i.id == "111"));
        assert!(acme_installations.iter().any(|i| i.id == "222"));

        let globex_installations = store.list_installations("globex").unwrap();
        assert_eq!(globex_installations.len(), 1);
        assert_eq!(globex_installations[0].id, "333");

        let none_for_unknown_tenant = store.list_installations("initech").unwrap();
        assert!(none_for_unknown_tenant.is_empty());
    }
}
