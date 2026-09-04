//! `PostgresGraphStore`: an optional `GraphStore` adapter for a shared,
//! multi-repo Postgres database — the "hypothetical future shared/
//! multi-tenant adapter" `agentops-graph`'s own module doc comment already
//! named as the reason repo-scoping is required on every trait method.
//! Part of `agentops-core`, available in every deployment. Schema:
//! `schema.sql` in this crate's root, a structural mirror of
//! `SqliteGraphStore`'s current schema, not a port of `main`'s stale
//! `agentops-heavy` one.
//!
//! **Sync bridge**: `GraphStore`'s trait methods are all synchronous
//! (matching `SqliteGraphStore`'s `rusqlite` calls), but `tokio-postgres`
//! is inherently async. Rather than making the trait itself async — which
//! would ripple through every existing caller (`agentops-mcp`,
//! `agentops-cli`, `agentops-api`, and all their tests) — this store owns a
//! `tokio::runtime::Runtime` internally and blocks on it per call. A call
//! therefore blocks its calling thread during I/O; an async caller
//! (`agentops-api`'s handlers) must wrap calls in `tokio::task::spawn_blocking`
//! to avoid stalling its executor, and must never call from inside an
//! already-running Tokio runtime directly (nested `block_on` panics).
//!
//! **Cheaply `Clone`, deliberately** — `connect()` used to be called fresh
//! on every HTTP request (a brand-new `Runtime` + connection pool per
//! call), which broke under real concurrent load: 54 simultaneous requests
//! meant 54 simultaneous pool creations competing for Postgres connections,
//! and 32 of them failed. Callers that live for the process's lifetime
//! (`agentops-heavy-api`/`agentops-api`'s `AppState`) now call `connect()`
//! once at startup and clone the result per request instead — cheap, since
//! `deadpool_postgres::Pool` is already `Arc`-backed internally and `rt` is
//! wrapped in `Arc` below purely so `Clone` is possible at all (`Runtime`
//! itself isn't `Clone`). See `agentops_mcp::store::open_shared_postgres_store`/
//! `resolve_store` for how callers actually get a shared instance.
//!
//! **Pool sizing** (`AGENTOPS_PG_POOL_MAX_SIZE`, default 25): grounded in
//! the standard Postgres pool-sizing guidance `(server_core_count * 2) +
//! effective_spindle_count` (Oracle's Real-World Performance Group,
//! corroborated across current PgBouncer/HikariCP-for-Postgres tuning
//! writeups) — 25 for a 12-core Postgres server on SSD storage. This is the
//! **Postgres server's** own core count, not this process's — deadpool's
//! own implicit default (`num_cpus::get() * 2`, used when unset) reflects
//! whatever container this Rust process happens to run in, which can
//! differ from the Postgres server's. Bigger is not automatically better:
//! the same research found shrinking an oversized pool dropped response
//! time from ~100ms to ~2ms — oversized pools measurably hurt latency, not
//! just waste memory. Override this env var only after profiling against
//! the real deployment's actual Postgres server core count/storage.

use agentops_embeddings::EMBEDDING_DIM;
use agentops_graph::{rank_notes_by_weight, Edge, EdgeRelation, GraphStore, NaturalKey, NewNode, NewScanHistoryEntry, NewSessionUsage, NewTask, Node, NodeKind, NodeProminence, NodeVersion, RepoState, ScanChange, ScanHistory, ScanHistoryEntry, SessionEvent, SessionUsage, Task, TaskLink, TaskStatus};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

const SCHEMA: &str = include_str!("../schema.sql");

#[derive(Clone)]
pub struct PostgresGraphStore {
    // Kept alive for the store's whole lifetime, not just at construction —
    // `deadpool-postgres`'s `Runtime::Tokio1`-mode pool needs an active
    // Tokio context for background connection recycling the entire time
    // it's in use. Dropping this right after building the pool would panic
    // on the first query issued afterward. `Arc`-wrapped so the whole
    // struct can be cheaply `Clone`d (see module doc comment) — `Runtime`
    // itself has no `Clone` impl.
    rt: Arc<RuntimeGuard>,
    pool: deadpool_postgres::Pool,
}

/// Wraps `Runtime` purely so its *last* `Arc` clone's drop calls
/// `shutdown_background()` instead of `Runtime`'s own default `Drop`, which
/// blocks waiting for every worker thread to finish -- disallowed ("Cannot
/// drop a runtime in a context where blocking is not allowed") when that
/// final drop happens to run on a thread that's itself inside another async
/// context, e.g. an `axum::Router` (and the `PostgresGraphStore` it
/// captured, all the way back through `AppState`) going out of scope inside
/// `agentops-server::run`'s own `#[tokio::main]` runtime on graceful
/// shutdown. Confirmed live: this exact panic fired from a regression test
/// added for the sibling "Cannot start a runtime from within a runtime"
/// incident, one layer deeper -- fixing the connect-time panic surfaced
/// this drop-time one right behind it. `shutdown_background()` returns
/// immediately without waiting for in-flight queries to finish, an
/// acceptable tradeoff for a pool that's being torn down anyway (unlike a
/// mid-request cancellation, nothing is waiting on the result).
struct RuntimeGuard(Option<tokio::runtime::Runtime>);

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_background();
        }
    }
}

// So every existing `self.rt.block_on(...)` call site keeps working
// unchanged through the `Arc<RuntimeGuard>` -- `Option::unwrap` is safe
// here since `0` is only ever `None` after `drop` has already run (nothing
// can call a method on a value mid-drop).
impl std::ops::Deref for RuntimeGuard {
    type Target = tokio::runtime::Runtime;
    fn deref(&self) -> &tokio::runtime::Runtime {
        self.0.as_ref().unwrap()
    }
}

/// Defaults to the standard `(postgres_server_cores * 2) + effective_spindle_count`
/// sizing guidance, not deadpool's implicit `num_cpus::get() * 2` (see this
/// module's own doc comment for why that default is the wrong basis here).
fn pg_pool_max_size() -> usize {
    std::env::var("AGENTOPS_PG_POOL_MAX_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(25)
}

impl PostgresGraphStore {
    /// Connects to `database_url` and runs the schema migration (idempotent
    /// — every statement in `schema.sql` is `IF NOT EXISTS`). No repo-scoped
    /// file path, unlike `SqliteGraphStore::open`: one Postgres database
    /// serves every repo, distinguished entirely via the `repo` column.
    pub fn connect(database_url: &str) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()?;
        let pool = rt.block_on(async {
            let mut cfg = deadpool_postgres::Config::new();
            cfg.url = Some(database_url.to_string());
            cfg.pool = Some(deadpool_postgres::PoolConfig::new(pg_pool_max_size()));
            let pool = cfg.create_pool(Some(deadpool_postgres::Runtime::Tokio1), tokio_postgres::NoTls)?;
            let client = pool.get().await?;
            client.batch_execute(SCHEMA).await?;
            Ok::<deadpool_postgres::Pool, anyhow::Error>(pool)
        })?;
        Ok(Self { rt: Arc::new(RuntimeGuard(Some(rt))), pool })
    }

    /// Deletes every row scoped to `repo` across every table — `nodes`
    /// (`edges` cascade automatically via the schema's `ON DELETE CASCADE`),
    /// plus the soft-referenced tables that don't cascade (`node_versions`,
    /// `task_links`, `scan_history_entries`) which would otherwise dangle
    /// against ids a subsequent fresh load reuses. Inherent, not a
    /// `GraphStore` trait method — this is a one-off wholesale-replace
    /// operation (used by `agentops-cli`'s `migrate-graph --wipe-target`),
    /// not a use case any normal caller needs.
    pub fn wipe_repo(&self, repo: &str) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            client.execute("DELETE FROM node_versions WHERE node_id IN (SELECT id FROM nodes WHERE repo = $1)", &[&repo]).await?;
            client.execute("DELETE FROM task_links WHERE node_id IN (SELECT id FROM nodes WHERE repo = $1) OR task_id IN (SELECT id FROM tasks WHERE repo = $1)", &[&repo]).await?;
            client.execute("DELETE FROM scan_history_entries WHERE scan_id IN (SELECT id FROM scan_history WHERE repo = $1)", &[&repo]).await?;
            client.execute("DELETE FROM tasks WHERE repo = $1", &[&repo]).await?;
            client.execute("DELETE FROM scan_history WHERE repo = $1", &[&repo]).await?;
            client.execute("DELETE FROM doc_pages WHERE repo = $1", &[&repo]).await?;
            client.execute("DELETE FROM repo_state WHERE repo = $1", &[&repo]).await?;
            client.execute("DELETE FROM session_events WHERE repo = $1", &[&repo]).await?;
            // Nodes last: edges cascade at the DB level, and the
            // soft-referenced tables above are already cleared, so nothing
            // left references these ids.
            client.execute("DELETE FROM nodes WHERE repo = $1", &[&repo]).await?;
            Ok(())
        })
    }
}

fn row_to_node(row: &tokio_postgres::Row) -> Node {
    let kind: String = row.get("kind");
    let prominence: String = row.get("prominence");
    Node {
        id: row.get("id"),
        kind: NodeKind::from_db_str(&kind),
        repo: row.get("repo"),
        path: row.get("path"),
        name: row.get("name"),
        container: row.get("container"),
        start_line: row.get("start_line"),
        end_line: row.get("end_line"),
        content: row.get("content"),
        curated: row.get("curated"),
        prominence: NodeProminence::from_db_str(&prominence),
        curation_reason: row.get("curation_reason"),
        // Initiative 3 (CLS-inspired retrieval plan): not wired up on this
        // backend yet -- see schema.sql's own note. `None` makes recency
        // ranking a no-op here rather than reading a wrong/absent column.
        last_touched_at: None,
    }
}

fn row_to_edge(row: &tokio_postgres::Row) -> Edge {
    let relation: String = row.get("relation");
    Edge {
        id: row.get("id"),
        repo: row.get("repo"),
        src_id: row.get("src_id"),
        dst_id: row.get("dst_id"),
        relation: EdgeRelation::from_db_str(&relation),
        weight: row.get("weight"),
        updated_at: row.get("updated_at"),
    }
}

fn row_to_repo_state(row: &tokio_postgres::Row) -> RepoState {
    let top_gotcha_ids: String = row.get("top_gotcha_ids");
    let top_decision_ids: String = row.get("top_decision_ids");
    RepoState {
        repo: row.get("repo"),
        updated_at: row.get("updated_at"),
        last_scan_id: row.get("last_scan_id"),
        top_gotcha_ids: serde_json::from_str(&top_gotcha_ids).unwrap_or_default(),
        top_decision_ids: serde_json::from_str(&top_decision_ids).unwrap_or_default(),
    }
}

// `weight`/`updated_at` are plain typed columns already (not a Postgres
// timestamp-vs-text mismatch the way `started_at` is), except `updated_at`
// itself: `TIMESTAMPTZ` needs the same `::text` cast `SCAN_HISTORY_COLUMNS`
// already established for `started_at`, so `row.get::<_, String>` works
// without a `chrono`/`time` dependency.
const EDGES_COLUMNS: &str = "id, repo, src_id, dst_id, relation, weight, updated_at::text AS updated_at";

fn row_to_node_version(row: &tokio_postgres::Row) -> NodeVersion {
    NodeVersion {
        id: row.get("id"),
        node_id: row.get("node_id"),
        content: row.get("content"),
        start_line: row.get("start_line"),
        end_line: row.get("end_line"),
        valid_from: row.get("valid_from"),
        valid_until: row.get("valid_until"),
    }
}

// Same `::text` cast reasoning as EDGES_COLUMNS — valid_until is nullable,
// and casting NULL::timestamptz to text stays NULL, so Option<String> reads
// back correctly either way.
const NODE_VERSIONS_COLUMNS: &str = "id, node_id, content, start_line, end_line, valid_from::text AS valid_from, valid_until::text AS valid_until";

fn row_to_session_event(row: &tokio_postgres::Row) -> SessionEvent {
    SessionEvent {
        id: row.get("id"),
        repo: row.get("repo"),
        session_id: row.get("session_id"),
        tool_name: row.get("tool_name"),
        description: row.get("description"),
        node_id: row.get("node_id"),
        event_kind: row.get("event_kind"),
        created_at: row.get("created_at"),
    }
}

// Same `::text` cast reasoning as EDGES_COLUMNS.
const SESSION_EVENTS_COLUMNS: &str = "id, repo, session_id, tool_name, description, node_id, event_kind, created_at::text AS created_at";

fn row_to_session_usage(row: &tokio_postgres::Row) -> SessionUsage {
    SessionUsage {
        id: row.get("id"),
        repo: row.get("repo"),
        session_id: row.get("session_id"),
        model: row.get("model"),
        input_tokens: row.get("input_tokens"),
        output_tokens: row.get("output_tokens"),
        cache_read_tokens: row.get("cache_read_tokens"),
        cache_write_tokens: row.get("cache_write_tokens"),
        cost_estimate_usd: row.get("cost_estimate_usd"),
        session_started_at: row.get("session_started_at"),
        session_ended_at: row.get("session_ended_at"),
        recorded_at: row.get("recorded_at"),
    }
}

// Same `::text` cast reasoning as EDGES_COLUMNS -- session_started_at/
// session_ended_at/recorded_at are TIMESTAMPTZ in Postgres but TEXT in
// SQLite, so the shared Rust `SessionUsage` struct needs both backends to
// hand back plain strings.
const SESSION_USAGE_COLUMNS: &str = "id, repo, session_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_estimate_usd, \
     session_started_at::text AS session_started_at, session_ended_at::text AS session_ended_at, recorded_at::text AS recorded_at";

fn row_to_task(row: &tokio_postgres::Row) -> Task {
    let status: String = row.get("status");
    Task {
        id: row.get("id"),
        repo: row.get("repo"),
        title: row.get("title"),
        description: row.get("description"),
        status: TaskStatus::from_db_str(&status),
        priority: row.get("priority"),
        assignee: row.get("assignee"),
        external_source: row.get("external_source"),
        external_id: row.get("external_id"),
        session_id: row.get("session_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

const TASKS_COLUMNS: &str =
    "id, repo, title, description, status, priority, assignee, external_source, external_id, session_id, created_at::text AS created_at, updated_at::text AS updated_at";

fn row_to_task_link(row: &tokio_postgres::Row) -> TaskLink {
    TaskLink { task_id: row.get("task_id"), node_id: row.get("node_id"), relation: row.get("relation") }
}

fn row_to_scan_history(row: &tokio_postgres::Row) -> ScanHistory {
    ScanHistory {
        id: row.get("id"),
        repo: row.get("repo"),
        started_at: row.get("started_at"),
        files_added: row.get("files_added"),
        files_changed: row.get("files_changed"),
        files_removed: row.get("files_removed"),
        symbols_added: row.get("symbols_added"),
        symbols_changed: row.get("symbols_changed"),
        symbols_removed: row.get("symbols_removed"),
        notes_added: row.get("notes_added"),
    }
}

const SCAN_HISTORY_COLUMNS: &str = "id, repo, started_at::text AS started_at, files_added, files_changed, files_removed, symbols_added, symbols_changed, symbols_removed, notes_added";

impl GraphStore for PostgresGraphStore {
    fn add_node(&self, node: NewNode) -> Result<i64> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_one(
                    "INSERT INTO nodes (kind, repo, path, name, container, start_line, end_line, content) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id",
                    &[&node.kind.as_db_str(), &node.repo, &node.path, &node.name, &node.container, &node.start_line, &node.end_line, &node.content],
                )
                .await?;
            Ok(row.get(0))
        })
    }

    fn get_node(&self, repo: &str, id: i64) -> Result<Option<Node>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client.query_opt("SELECT * FROM nodes WHERE repo = $1 AND id = $2", &[&repo, &id]).await?;
            Ok(row.as_ref().map(row_to_node))
        })
    }

    fn nodes_by_kind(&self, repo: &str, kind: NodeKind) -> Result<Vec<Node>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT * FROM nodes WHERE repo = $1 AND kind = $2", &[&repo, &kind.as_db_str()]).await?;
            Ok(rows.iter().map(row_to_node).collect())
        })
    }

    fn all_nodes(&self, repo: &str) -> Result<Vec<Node>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT * FROM nodes WHERE repo = $1", &[&repo]).await?;
            Ok(rows.iter().map(row_to_node).collect())
        })
    }

    fn find_node(&self, repo: &str, kind: NodeKind, path: Option<&str>, name: Option<&str>, container: Option<&str>) -> Result<Option<Node>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_opt(
                    "SELECT * FROM nodes WHERE repo = $1 AND kind = $2 AND path IS NOT DISTINCT FROM $3 AND name IS NOT DISTINCT FROM $4 AND container IS NOT DISTINCT FROM $5",
                    &[&repo, &kind.as_db_str(), &path, &name, &container],
                )
                .await?;
            Ok(row.as_ref().map(row_to_node))
        })
    }

    fn update_node(&self, repo: &str, id: i64, start_line: Option<i64>, end_line: Option<i64>, content: Option<String>) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            client.execute("UPDATE nodes SET start_line = $1, end_line = $2, content = $3 WHERE repo = $4 AND id = $5", &[&start_line, &end_line, &content, &repo, &id]).await?;
            Ok(())
        })
    }

    fn set_curation(&self, repo: &str, node_id: i64, prominence: NodeProminence, reason: Option<&str>) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            client.execute("UPDATE nodes SET curated = true, prominence = $1, curation_reason = $2 WHERE repo = $3 AND id = $4", &[&prominence.as_db_str(), &reason, &repo, &node_id]).await?;
            Ok(())
        })
    }

    fn delete_nodes(&self, repo: &str, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            // Edges cascade (schema's `ON DELETE CASCADE`), and `embedding`
            // is a plain column on `nodes` itself — unlike SqliteGraphStore
            // (a separate `vec0` virtual table it must explicitly clean up
            // to avoid orphan rows), there's nothing else to delete here.
            client.execute("DELETE FROM nodes WHERE repo = $1 AND id = ANY($2)", &[&repo, &ids]).await?;
            Ok(())
        })
    }

    fn add_edge(&self, repo: &str, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_one(
                    "INSERT INTO edges (repo, src_id, dst_id, relation, weight, updated_at) VALUES ($1,$2,$3,$4,1.0,now()) RETURNING id",
                    &[&repo, &src_id, &dst_id, &relation.as_db_str()],
                )
                .await?;
            Ok(row.get(0))
        })
    }

    fn reinforce_edge(&self, repo: &str, edge_id: i64, bump_confirmed_at: bool) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = if bump_confirmed_at {
                "UPDATE edges SET weight = LEAST(weight + 0.5, 5.0), updated_at = now() WHERE repo = $1 AND id = $2"
            } else {
                "UPDATE edges SET weight = LEAST(weight + 0.5, 5.0) WHERE repo = $1 AND id = $2"
            };
            client.execute(sql, &[&repo, &edge_id]).await?;
            Ok(())
        })
    }

    fn edges_from(&self, repo: &str, src_id: i64) -> Result<Vec<Edge>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {EDGES_COLUMNS} FROM edges WHERE repo = $1 AND src_id = $2");
            let rows = client.query(&sql, &[&repo, &src_id]).await?;
            Ok(rows.iter().map(row_to_edge).collect())
        })
    }

    fn edges_to(&self, repo: &str, dst_id: i64) -> Result<Vec<Edge>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {EDGES_COLUMNS} FROM edges WHERE repo = $1 AND dst_id = $2");
            let rows = client.query(&sql, &[&repo, &dst_id]).await?;
            Ok(rows.iter().map(row_to_edge).collect())
        })
    }

    fn all_edges(&self, repo: &str) -> Result<Vec<Edge>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {EDGES_COLUMNS} FROM edges WHERE repo = $1");
            let rows = client.query(&sql, &[&repo]).await?;
            Ok(rows.iter().map(row_to_edge).collect())
        })
    }

    fn delete_edges_from(&self, repo: &str, src_id: i64, relation: EdgeRelation) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            client.execute("DELETE FROM edges WHERE repo = $1 AND src_id = $2 AND relation = $3", &[&repo, &src_id, &relation.as_db_str()]).await?;
            Ok(())
        })
    }

    fn delete_edge(&self, repo: &str, edge_id: i64) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            client.execute("DELETE FROM edges WHERE repo = $1 AND id = $2", &[&repo, &edge_id]).await?;
            Ok(())
        })
    }

    fn record_scan(&self, repo: &str, entries: &[NewScanHistoryEntry]) -> Result<i64> {
        self.rt.block_on(async {
            let mut client = self.pool.get().await?;
            let txn = client.transaction().await?;

            let count = |kind: NodeKind, change: ScanChange| entries.iter().filter(|e| e.kind == kind && e.change == change).count() as i64;
            let files_added = count(NodeKind::File, ScanChange::Added);
            let files_changed = count(NodeKind::File, ScanChange::Changed);
            let files_removed = count(NodeKind::File, ScanChange::Removed);
            let symbols_added = count(NodeKind::Symbol, ScanChange::Added);
            let symbols_changed = count(NodeKind::Symbol, ScanChange::Changed);
            let symbols_removed = count(NodeKind::Symbol, ScanChange::Removed);
            let notes_added = entries.iter().filter(|e| matches!(e.kind, NodeKind::Gotcha | NodeKind::Decision | NodeKind::Note) && e.change == ScanChange::Added).count() as i64;

            let row = txn
                .query_one(
                    "INSERT INTO scan_history (repo, files_added, files_changed, files_removed, symbols_added, symbols_changed, symbols_removed, notes_added) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id",
                    &[&repo, &files_added, &files_changed, &files_removed, &symbols_added, &symbols_changed, &symbols_removed, &notes_added],
                )
                .await?;
            let scan_id: i64 = row.get(0);

            for entry in entries {
                txn.execute(
                    "INSERT INTO scan_history_entries (scan_id, node_id, kind, path, name, change) VALUES ($1,$2,$3,$4,$5,$6)",
                    &[&scan_id, &entry.node_id, &entry.kind.as_db_str(), &entry.path, &entry.name, &entry.change.as_db_str()],
                )
                .await?;
            }

            txn.commit().await?;
            Ok(scan_id)
        })
    }

    fn latest_scan(&self, repo: &str) -> Result<Option<ScanHistory>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {SCAN_HISTORY_COLUMNS} FROM scan_history WHERE repo = $1 ORDER BY started_at DESC, id DESC LIMIT 1");
            let row = client.query_opt(&sql, &[&repo]).await?;
            Ok(row.as_ref().map(row_to_scan_history))
        })
    }

    fn list_scans(&self, repo: &str) -> Result<Vec<ScanHistory>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {SCAN_HISTORY_COLUMNS} FROM scan_history WHERE repo = $1 ORDER BY started_at DESC, id DESC");
            let rows = client.query(&sql, &[&repo]).await?;
            Ok(rows.iter().map(row_to_scan_history).collect())
        })
    }

    fn scan_entries(&self, scan_id: i64) -> Result<Vec<ScanHistoryEntry>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT * FROM scan_history_entries WHERE scan_id = $1", &[&scan_id]).await?;
            Ok(rows
                .iter()
                .map(|row| {
                    let kind: String = row.get("kind");
                    let change: String = row.get("change");
                    ScanHistoryEntry {
                        id: row.get("id"),
                        scan_id: row.get("scan_id"),
                        node_id: row.get("node_id"),
                        kind: NodeKind::from_db_str(&kind),
                        path: row.get("path"),
                        name: row.get("name"),
                        change: ScanChange::from_db_str(&change),
                    }
                })
                .collect())
        })
    }

    fn set_embedding(&self, repo: &str, node_id: i64, embedding: &[f32]) -> Result<()> {
        anyhow::ensure!(embedding.len() == EMBEDDING_DIM, "embedding has {} dims, expected {EMBEDDING_DIM}", embedding.len());
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let vector = pgvector::Vector::from(embedding.to_vec());
            let updated = client.execute("UPDATE nodes SET embedding = $1 WHERE repo = $2 AND id = $3", &[&vector, &repo, &node_id]).await?;
            anyhow::ensure!(updated == 1, "node #{node_id} not found in repo {repo:?}");
            Ok(())
        })
    }

    fn get_embedding(&self, repo: &str, node_id: i64) -> Result<Option<Vec<f32>>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client.query_opt("SELECT embedding FROM nodes WHERE repo = $1 AND id = $2", &[&repo, &node_id]).await?;
            let Some(row) = row else { return Ok(None) };
            let vector: Option<pgvector::Vector> = row.get("embedding");
            Ok(vector.map(|v| v.to_vec()))
        })
    }

    fn search_similar(&self, repo: &str, embedding: &[f32], top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>> {
        anyhow::ensure!(embedding.len() == EMBEDDING_DIM, "query embedding has {} dims, expected {EMBEDDING_DIM}", embedding.len());
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let vector = pgvector::Vector::from(embedding.to_vec());
            let top_k = top_k as i64;

            let rows = match kind {
                Some(k) => {
                    client
                        .query(
                            "SELECT *, embedding <=> $1 AS distance FROM nodes WHERE repo = $2 AND kind = $3 AND embedding IS NOT NULL ORDER BY embedding <=> $1 LIMIT $4",
                            &[&vector, &repo, &k.as_db_str(), &top_k],
                        )
                        .await?
                }
                None => {
                    client
                        .query("SELECT *, embedding <=> $1 AS distance FROM nodes WHERE repo = $2 AND embedding IS NOT NULL ORDER BY embedding <=> $1 LIMIT $3", &[&vector, &repo, &top_k])
                        .await?
                }
            };

            Ok(rows
                .iter()
                .map(|row| {
                    let node = row_to_node(row);
                    let distance: f64 = row.get("distance");
                    (node, distance as f32)
                })
                .collect())
        })
    }

    fn search_lexical(&self, repo: &str, query: &str, top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let top_k = top_k as i64;
            let rows = match kind {
                Some(k) => {
                    client
                        .query(
                            "SELECT *, ts_rank(search_vector, plainto_tsquery('english', $1)) AS rank FROM nodes \
                             WHERE repo = $2 AND kind = $3 AND search_vector @@ plainto_tsquery('english', $1) \
                             ORDER BY rank DESC LIMIT $4",
                            &[&query, &repo, &k.as_db_str(), &top_k],
                        )
                        .await?
                }
                None => {
                    client
                        .query(
                            "SELECT *, ts_rank(search_vector, plainto_tsquery('english', $1)) AS rank FROM nodes \
                             WHERE repo = $2 AND search_vector @@ plainto_tsquery('english', $1) \
                             ORDER BY rank DESC LIMIT $3",
                            &[&query, &repo, &top_k],
                        )
                        .await?
                }
            };
            Ok(rows.iter().map(|row| (row_to_node(row), row.get::<_, f32>("rank"))).collect())
        })
    }

    fn search_exact(&self, repo: &str, query: &str, top_k: usize, kind: Option<NodeKind>) -> Result<Vec<(Node, f32)>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let top_k = top_k as i64;
            let rows = client
                .query(
                    "SELECT * FROM nodes WHERE repo = $1 AND ($2::text IS NULL OR kind = $2) AND (LOWER(name) = LOWER($3) OR LOWER(name) LIKE '%' || LOWER($3) || '%') \
                     ORDER BY CASE WHEN LOWER(name) = LOWER($3) THEN 0 ELSE 1 END, LENGTH(name) LIMIT $4",
                    &[&repo, &kind.map(|k| k.as_db_str()), &query, &top_k],
                )
                .await?;
            Ok(rows
                .iter()
                .map(|row| {
                    let node = row_to_node(row);
                    let exact = node.name.as_deref().is_some_and(|name| name.eq_ignore_ascii_case(query));
                    (node, if exact { 0.0 } else { 1.0 })
                })
                .collect())
        })
    }

    fn refresh_repo_state(&self, repo: &str) -> Result<RepoState> {
        // Ranking uses the exact same `rank_notes_by_weight` free function
        // `SqliteGraphStore` calls — computed in Rust, not in SQL, so the
        // two backends can never silently diverge on what "top" means.
        let top_gotcha_ids = rank_notes_by_weight(self, repo, NodeKind::Gotcha)?;
        let top_decision_ids = rank_notes_by_weight(self, repo, NodeKind::Decision)?;
        let last_scan_id = self.latest_scan(repo)?.map(|s| s.id);
        let gotcha_json = serde_json::to_string(&top_gotcha_ids)?;
        let decision_json = serde_json::to_string(&top_decision_ids)?;

        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_one(
                    "INSERT INTO repo_state (repo, updated_at, last_scan_id, top_gotcha_ids, top_decision_ids)
                     VALUES ($1, now(), $2, $3, $4)
                     ON CONFLICT (repo) DO UPDATE SET
                        updated_at = excluded.updated_at,
                        last_scan_id = excluded.last_scan_id,
                        top_gotcha_ids = excluded.top_gotcha_ids,
                        top_decision_ids = excluded.top_decision_ids
                     RETURNING updated_at::text",
                    &[&repo, &last_scan_id, &gotcha_json, &decision_json],
                )
                .await?;
            let updated_at: String = row.get(0);
            Ok(RepoState { repo: repo.to_string(), updated_at, last_scan_id, top_gotcha_ids, top_decision_ids })
        })
    }

    fn get_repo_state(&self, repo: &str) -> Result<Option<RepoState>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_opt("SELECT repo, updated_at::text AS updated_at, last_scan_id, top_gotcha_ids, top_decision_ids FROM repo_state WHERE repo = $1", &[&repo])
                .await?;
            Ok(row.as_ref().map(row_to_repo_state))
        })
    }

    fn save_doc_page(&self, repo: &str, generated_at: &str, content_json: &str) -> Result<()> {
        // `generated_at` comes from the caller (it's the same
        // `ScanHistory.started_at` text value already embedded inside
        // `content_json`'s `DocPage.generated_at` field — see
        // `agentops-mcp`'s orchestration) rather than a fresh `now()` here,
        // so the DB column and the JSON blob's own field can never drift
        // apart. `::timestamptz` parses the TEXT scan timestamp — but the
        // parameter itself must still be sent as TEXT on the wire (an
        // explicit `prepare_typed`, not left to the driver's own inference
        // from the `::timestamptz` cast site, which describes the
        // placeholder itself as `timestamptz` and then rejects a `&str`
        // value outright with "cannot convert between the Rust type `&str`
        // and the Postgres type `timestamptz`" — caught live via
        // `migrate-graph`'s dry run, this path had never actually been
        // exercised against Postgres before).
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let stmt = client
                .prepare_typed(
                    "INSERT INTO doc_pages (repo, generated_at, content)
                     VALUES ($1, $2::timestamptz, $3)
                     ON CONFLICT (repo) DO UPDATE SET
                        generated_at = excluded.generated_at,
                        content = excluded.content",
                    &[tokio_postgres::types::Type::TEXT, tokio_postgres::types::Type::TEXT, tokio_postgres::types::Type::TEXT],
                )
                .await?;
            client.execute(&stmt, &[&repo, &generated_at, &content_json]).await?;
            Ok(())
        })
    }

    fn get_doc_page(&self, repo: &str) -> Result<Option<(String, String)>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client.query_opt("SELECT generated_at::text AS generated_at, content FROM doc_pages WHERE repo = $1", &[&repo]).await?;
            Ok(row.as_ref().map(|r| (r.get::<_, String>(0), r.get::<_, String>(1))))
        })
    }

    fn snapshot_node_version(&self, node_id: i64, content: Option<&str>, start_line: Option<i64>, end_line: Option<i64>) -> Result<()> {
        self.rt.block_on(async {
            let mut client = self.pool.get().await?;
            let txn = client.transaction().await?;
            txn.execute("UPDATE node_versions SET valid_until = now() WHERE node_id = $1 AND valid_until IS NULL", &[&node_id]).await?;
            txn.execute(
                "INSERT INTO node_versions (node_id, content, start_line, end_line, valid_from, valid_until) VALUES ($1, $2, $3, $4, now(), NULL)",
                &[&node_id, &content, &start_line, &end_line],
            )
            .await?;
            txn.commit().await?;
            Ok(())
        })
    }

    fn close_node_version(&self, node_id: i64) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            client.execute("UPDATE node_versions SET valid_until = now() WHERE node_id = $1 AND valid_until IS NULL", &[&node_id]).await?;
            Ok(())
        })
    }

    fn node_history(&self, node_id: i64) -> Result<Vec<NodeVersion>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {NODE_VERSIONS_COLUMNS} FROM node_versions WHERE node_id = $1 ORDER BY id DESC");
            let rows = client.query(&sql, &[&node_id]).await?;
            Ok(rows.iter().map(row_to_node_version).collect())
        })
    }

    fn node_as_of(&self, node_id: i64, timestamp: &str) -> Result<Option<NodeVersion>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!(
                "SELECT {NODE_VERSIONS_COLUMNS} FROM node_versions \
                 WHERE node_id = $1 AND valid_from <= $2::timestamptz AND (valid_until IS NULL OR valid_until > $2::timestamptz) \
                 ORDER BY id DESC LIMIT 1"
            );
            // Same `prepare_typed` fix as `save_doc_page`: the `::timestamptz`
            // cast makes the driver describe $2 as `timestamptz`, which a
            // `&str` value can't satisfy directly.
            let stmt = client.prepare_typed(&sql, &[tokio_postgres::types::Type::INT8, tokio_postgres::types::Type::TEXT]).await?;
            let row = client.query_opt(&stmt, &[&node_id, &timestamp]).await?;
            Ok(row.as_ref().map(row_to_node_version))
        })
    }

    fn record_session_event(&self, repo: &str, session_id: &str, tool_name: &str, description: &str, node_id: Option<i64>, event_kind: &str) -> Result<i64> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_one(
                    "INSERT INTO session_events (repo, session_id, tool_name, description, node_id, event_kind) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
                    &[&repo, &session_id, &tool_name, &description, &node_id, &event_kind],
                )
                .await?;
            Ok(row.get(0))
        })
    }

    fn session_events(&self, repo: &str, session_id: &str) -> Result<Vec<SessionEvent>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {SESSION_EVENTS_COLUMNS} FROM session_events WHERE repo = $1 AND session_id = $2 ORDER BY id ASC");
            let rows = client.query(&sql, &[&repo, &session_id]).await?;
            Ok(rows.iter().map(row_to_session_event).collect())
        })
    }

    fn session_events_for_repo(&self, repo: &str, event_kind: Option<&str>) -> Result<Vec<SessionEvent>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let rows = match event_kind {
                Some(kind) => {
                    let sql = format!("SELECT {SESSION_EVENTS_COLUMNS} FROM session_events WHERE repo = $1 AND event_kind = $2 ORDER BY id ASC");
                    client.query(&sql, &[&repo, &kind]).await?
                }
                None => {
                    let sql = format!("SELECT {SESSION_EVENTS_COLUMNS} FROM session_events WHERE repo = $1 ORDER BY id ASC");
                    client.query(&sql, &[&repo]).await?
                }
            };
            Ok(rows.iter().map(row_to_session_event).collect())
        })
    }

    fn upsert_session_usage(&self, usage: NewSessionUsage) -> Result<i64> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = "INSERT INTO session_usage (repo, session_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_estimate_usd, session_started_at, session_ended_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::timestamptz, $10::timestamptz) \
                 ON CONFLICT (repo, session_id, model) DO UPDATE SET \
                    input_tokens = excluded.input_tokens, \
                    output_tokens = excluded.output_tokens, \
                    cache_read_tokens = excluded.cache_read_tokens, \
                    cache_write_tokens = excluded.cache_write_tokens, \
                    cost_estimate_usd = excluded.cost_estimate_usd, \
                    session_started_at = excluded.session_started_at, \
                    session_ended_at = excluded.session_ended_at, \
                    recorded_at = now() \
                 RETURNING id";
            let row = client
                .query_one(
                    sql,
                    &[
                        &usage.repo,
                        &usage.session_id,
                        &usage.model,
                        &usage.input_tokens,
                        &usage.output_tokens,
                        &usage.cache_read_tokens,
                        &usage.cache_write_tokens,
                        &usage.cost_estimate_usd,
                        &usage.session_started_at,
                        &usage.session_ended_at,
                    ],
                )
                .await?;
            Ok(row.get(0))
        })
    }

    fn session_usage_for_repo(&self, repo: &str) -> Result<Vec<SessionUsage>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {SESSION_USAGE_COLUMNS} FROM session_usage WHERE repo = $1 ORDER BY session_started_at DESC");
            let rows = client.query(&sql, &[&repo]).await?;
            Ok(rows.iter().map(row_to_session_usage).collect())
        })
    }

    fn create_task(&self, task: NewTask) -> Result<i64> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let status = task.status.as_db_str();
            let row = client
                .query_one(
                    "INSERT INTO tasks (repo, title, description, status, priority, assignee, external_source, external_id, session_id) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
                    &[&task.repo, &task.title, &task.description, &status, &task.priority, &task.assignee, &task.external_source, &task.external_id, &task.session_id],
                )
                .await?;
            Ok(row.get(0))
        })
    }

    fn get_task(&self, id: i64) -> Result<Option<Task>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {TASKS_COLUMNS} FROM tasks WHERE id = $1");
            let row = client.query_opt(&sql, &[&id]).await?;
            Ok(row.as_ref().map(row_to_task))
        })
    }

    fn list_tasks(&self, repo: &str) -> Result<Vec<Task>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {TASKS_COLUMNS} FROM tasks WHERE repo = $1 ORDER BY id ASC");
            let rows = client.query(&sql, &[&repo]).await?;
            Ok(rows.iter().map(row_to_task).collect())
        })
    }

    fn update_task_status(&self, id: i64, status: TaskStatus) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let status = status.as_db_str();
            client.execute("UPDATE tasks SET status = $1, updated_at = now() WHERE id = $2", &[&status, &id]).await?;
            Ok(())
        })
    }

    // Same reasoning as SqliteGraphStore's impl: a manual find-then-branch,
    // not a generic ON CONFLICT DO UPDATE, so created_at survives a resync.
    fn upsert_external_task(&self, task: NewTask) -> Result<i64> {
        anyhow::ensure!(task.external_source.is_some() && task.external_id.is_some(), "upsert_external_task requires both external_source and external_id");
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let existing = client.query_opt("SELECT id FROM tasks WHERE external_source = $1 AND external_id = $2", &[&task.external_source, &task.external_id]).await?;

            match existing {
                Some(row) => {
                    let id: i64 = row.get(0);
                    let status = task.status.as_db_str();
                    client
                        .execute(
                            "UPDATE tasks SET repo = $1, title = $2, description = $3, status = $4, priority = $5, assignee = $6, session_id = $7, updated_at = now() WHERE id = $8",
                            &[&task.repo, &task.title, &task.description, &status, &task.priority, &task.assignee, &task.session_id, &id],
                        )
                        .await?;
                    Ok(id)
                }
                None => {
                    let status = task.status.as_db_str();
                    let row = client
                        .query_one(
                            "INSERT INTO tasks (repo, title, description, status, priority, assignee, external_source, external_id, session_id) \
                             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
                            &[&task.repo, &task.title, &task.description, &status, &task.priority, &task.assignee, &task.external_source, &task.external_id, &task.session_id],
                        )
                        .await?;
                    Ok(row.get(0))
                }
            }
        })
    }

    fn link_task(&self, task_id: i64, node_id: i64, relation: &str) -> Result<()> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            client.execute("INSERT INTO task_links (task_id, node_id, relation) VALUES ($1, $2, $3) ON CONFLICT (task_id, node_id, relation) DO NOTHING", &[&task_id, &node_id, &relation]).await?;
            Ok(())
        })
    }

    fn task_links(&self, task_id: i64) -> Result<Vec<TaskLink>> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT task_id, node_id, relation FROM task_links WHERE task_id = $1", &[&task_id]).await?;
            Ok(rows.iter().map(row_to_task_link).collect())
        })
    }

    // -- Batch overrides (Postgres pool/batching plan, Phase 2) --
    //
    // Real multi-row SQL, replacing the trait's loop-based defaults --
    // `scan.rs::persist`'s intended caller pays one round trip per phase
    // instead of one per row. Same `self.rt.block_on` + `self.pool.get()`
    // pattern every other method here already uses, no new concurrency
    // primitive.

    fn find_nodes_batch(&self, repo: &str, keys: &[(NodeKind, Option<&str>, Option<&str>, Option<&str>)]) -> Result<HashMap<NaturalKey, Node>> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let kinds: Vec<&str> = keys.iter().map(|k| k.0.as_db_str()).collect();
            let paths: Vec<Option<&str>> = keys.iter().map(|k| k.1).collect();
            let names: Vec<Option<&str>> = keys.iter().map(|k| k.2).collect();
            let containers: Vec<Option<&str>> = keys.iter().map(|k| k.3).collect();
            // Same `IS NOT DISTINCT FROM` semantics as `find_node` itself
            // (see that method's own comment) -- a plain `=` join would
            // wrongly fail to match two keys that both have a NULL
            // path/name/container against each other.
            let rows = client
                .query(
                    "SELECT n.* FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[]) AS k(kind, path, name, container)
                     JOIN nodes n ON n.repo = $1 AND n.kind = k.kind
                         AND n.path IS NOT DISTINCT FROM k.path
                         AND n.name IS NOT DISTINCT FROM k.name
                         AND n.container IS NOT DISTINCT FROM k.container",
                    &[&repo, &kinds, &paths, &names, &containers],
                )
                .await?;
            let mut out = HashMap::new();
            for row in &rows {
                let node = row_to_node(row);
                out.insert((node.kind, node.path.clone(), node.name.clone(), node.container.clone()), node);
            }
            Ok(out)
        })
    }

    fn upsert_nodes_batch(&self, nodes: &[NewNode]) -> Result<Vec<i64>> {
        if nodes.is_empty() {
            return Ok(vec![]);
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;

            // Deduped, last-wins per natural key -- confirmed live against
            // real production data that the trait method's own documented
            // "no duplicate natural key within a batch" precondition does
            // NOT always hold in practice (the scanner can legitimately
            // emit two `Symbol` entries sharing the same `(path, name,
            // container)` for one file), and `ON CONFLICT DO UPDATE`
            // refuses to affect the same row twice within one statement
            // ("ON CONFLICT DO UPDATE command cannot affect row a second
            // time" -- a real Postgres error this exact scenario produced
            // in production, not a hypothetical). Last-wins matches the
            // trait default's own loop-based semantics: each subsequent
            // `upsert_node` call for the same key overwrites the previous
            // one's `start_line`/`end_line`/`content`. Only the *insert*
            // arrays are deduped -- the id-lookup query below still joins
            // against the full, non-deduped `nodes` slice, since every
            // duplicate-keyed input position resolves to the same
            // underlying row either way, so this doesn't break the
            // input-order-preserving contract of this method's return value.
            let mut dedup: HashMap<(&str, &str, Option<&str>, Option<&str>, Option<&str>), &NewNode> = HashMap::new();
            for n in nodes {
                dedup.insert((n.repo.as_str(), n.kind.as_db_str(), n.path.as_deref(), n.name.as_deref(), n.container.as_deref()), n);
            }
            let deduped: Vec<&NewNode> = dedup.into_values().collect();

            let repos: Vec<&str> = deduped.iter().map(|n| n.repo.as_str()).collect();
            let kinds: Vec<&str> = deduped.iter().map(|n| n.kind.as_db_str()).collect();
            let paths: Vec<Option<&str>> = deduped.iter().map(|n| n.path.as_deref()).collect();
            let names: Vec<Option<&str>> = deduped.iter().map(|n| n.name.as_deref()).collect();
            let containers: Vec<Option<&str>> = deduped.iter().map(|n| n.container.as_deref()).collect();
            let start_lines: Vec<Option<i64>> = deduped.iter().map(|n| n.start_line).collect();
            let end_lines: Vec<Option<i64>> = deduped.iter().map(|n| n.end_line).collect();
            let contents: Vec<Option<&str>> = deduped.iter().map(|n| n.content.as_deref()).collect();

            client
                .execute(
                    "INSERT INTO nodes (repo, kind, path, name, container, start_line, end_line, content)
                     SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], $6::bigint[], $7::bigint[], $8::text[])
                     ON CONFLICT (repo, kind, COALESCE(path, ''), COALESCE(name, ''), COALESCE(container, ''))
                     DO UPDATE SET start_line = EXCLUDED.start_line, end_line = EXCLUDED.end_line, content = EXCLUDED.content",
                    &[&repos, &kinds, &paths, &names, &containers, &start_lines, &end_lines, &contents],
                )
                .await?;

            // A second round trip to recover ids in input order -- against
            // the *full*, non-deduped input slice, so every original
            // position (including duplicate-keyed ones) gets an id back.
            // `RETURNING` on an `INSERT ... SELECT` can only return columns
            // of the target table, not the source query's own row number,
            // so ids can't be pulled straight off the statement above.
            // Still O(1) round trips for the whole batch, not O(n) -- and
            // guaranteed to find exactly one row per input node, since
            // `idx_nodes_natural_key` means the insert above just created
            // or updated exactly one row per distinct natural key.
            let repos: Vec<&str> = nodes.iter().map(|n| n.repo.as_str()).collect();
            let kinds: Vec<&str> = nodes.iter().map(|n| n.kind.as_db_str()).collect();
            let paths: Vec<Option<&str>> = nodes.iter().map(|n| n.path.as_deref()).collect();
            let names: Vec<Option<&str>> = nodes.iter().map(|n| n.name.as_deref()).collect();
            let containers: Vec<Option<&str>> = nodes.iter().map(|n| n.container.as_deref()).collect();
            let rows = client
                .query(
                    "SELECT k.ord, n.id FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[]) WITH ORDINALITY AS k(repo, kind, path, name, container, ord)
                     JOIN nodes n ON n.repo = k.repo AND n.kind = k.kind
                         AND n.path IS NOT DISTINCT FROM k.path
                         AND n.name IS NOT DISTINCT FROM k.name
                         AND n.container IS NOT DISTINCT FROM k.container",
                    &[&repos, &kinds, &paths, &names, &containers],
                )
                .await?;

            let mut id_by_ord: HashMap<i64, i64> = HashMap::new();
            for row in &rows {
                id_by_ord.insert(row.get::<_, i64>("ord"), row.get::<_, i64>("id"));
            }
            (1..=nodes.len() as i64)
                .map(|ord| id_by_ord.get(&ord).copied().ok_or_else(|| anyhow::anyhow!("upsert_nodes_batch: no row found for input #{ord} after insert -- unexpected")))
                .collect()
        })
    }

    fn add_edges_batch(&self, repo: &str, edges: &[(i64, i64, EdgeRelation)]) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let src_ids: Vec<i64> = edges.iter().map(|e| e.0).collect();
            let dst_ids: Vec<i64> = edges.iter().map(|e| e.1).collect();
            let relations: Vec<&str> = edges.iter().map(|e| e.2.as_db_str()).collect();
            client
                .execute(
                    "INSERT INTO edges (repo, src_id, dst_id, relation, weight, updated_at)
                     SELECT $1, src_id, dst_id, relation, 1.0, now()
                     FROM UNNEST($2::bigint[], $3::bigint[], $4::text[]) AS t(src_id, dst_id, relation)",
                    &[&repo, &src_ids, &dst_ids, &relations],
                )
                .await?;
            Ok(())
        })
    }

    fn set_embeddings_batch(&self, repo: &str, embeddings: &[(i64, Vec<f32>)]) -> Result<()> {
        if embeddings.is_empty() {
            return Ok(());
        }
        for (_, e) in embeddings {
            anyhow::ensure!(e.len() == EMBEDDING_DIM, "embedding has {} dims, expected {EMBEDDING_DIM}", e.len());
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let ids: Vec<i64> = embeddings.iter().map(|(id, _)| *id).collect();
            let vectors: Vec<pgvector::Vector> = embeddings.iter().map(|(_, e)| pgvector::Vector::from(e.clone())).collect();
            let updated = client
                .execute(
                    "UPDATE nodes n SET embedding = t.embedding
                     FROM UNNEST($2::bigint[], $3::vector[]) AS t(id, embedding)
                     WHERE n.repo = $1 AND n.id = t.id",
                    &[&repo, &ids, &vectors],
                )
                .await?;
            anyhow::ensure!(updated as usize == embeddings.len(), "set_embeddings_batch: expected to update {} rows, updated {updated} -- some node ids weren't found in repo {repo:?}", embeddings.len());
            Ok(())
        })
    }

    fn snapshot_node_versions_batch(&self, repo: &str, versions: &[(i64, Option<&str>, Option<i64>, Option<i64>)]) -> Result<()> {
        let _ = repo;
        if versions.is_empty() {
            return Ok(());
        }
        self.rt.block_on(async {
            let mut client = self.pool.get().await?;
            let txn = client.transaction().await?;
            let node_ids: Vec<i64> = versions.iter().map(|v| v.0).collect();
            let contents: Vec<Option<&str>> = versions.iter().map(|v| v.1).collect();
            let start_lines: Vec<Option<i64>> = versions.iter().map(|v| v.2).collect();
            let end_lines: Vec<Option<i64>> = versions.iter().map(|v| v.3).collect();

            txn.execute("UPDATE node_versions SET valid_until = now() WHERE node_id = ANY($1) AND valid_until IS NULL", &[&node_ids]).await?;
            txn.execute(
                "INSERT INTO node_versions (node_id, content, start_line, end_line, valid_from, valid_until)
                 SELECT node_id, content, start_line, end_line, now(), NULL
                 FROM UNNEST($1::bigint[], $2::text[], $3::bigint[], $4::bigint[]) AS t(node_id, content, start_line, end_line)",
                &[&node_ids, &contents, &start_lines, &end_lines],
            )
            .await?;
            txn.commit().await?;
            Ok(())
        })
    }

    fn edges_from_batch(&self, repo: &str, src_ids: &[i64]) -> Result<HashMap<i64, Vec<Edge>>> {
        // Every requested src_id must appear in the result, mapped to an
        // empty Vec if it has no edges -- same "always present" contract
        // the trait default (looping `edges_from`) already has.
        let mut out: HashMap<i64, Vec<Edge>> = src_ids.iter().map(|&id| (id, Vec::new())).collect();
        if src_ids.is_empty() {
            return Ok(out);
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = format!("SELECT {EDGES_COLUMNS} FROM edges WHERE repo = $1 AND src_id = ANY($2)");
            let rows = client.query(&sql, &[&repo, &src_ids]).await?;
            for row in &rows {
                let edge = row_to_edge(row);
                out.entry(edge.src_id).or_default().push(edge);
            }
            Ok(out)
        })
    }

    fn reinforce_edges_batch(&self, repo: &str, edge_ids: &[i64], bump_confirmed_at: bool) -> Result<()> {
        if edge_ids.is_empty() {
            return Ok(());
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let sql = if bump_confirmed_at {
                "UPDATE edges SET weight = LEAST(weight + 0.5, 5.0), updated_at = now() WHERE repo = $1 AND id = ANY($2)"
            } else {
                "UPDATE edges SET weight = LEAST(weight + 0.5, 5.0) WHERE repo = $1 AND id = ANY($2)"
            };
            client.execute(sql, &[&repo, &edge_ids]).await?;
            Ok(())
        })
    }

    fn delete_edges_batch(&self, repo: &str, edge_ids: &[i64]) -> Result<()> {
        if edge_ids.is_empty() {
            return Ok(());
        }
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            client.execute("DELETE FROM edges WHERE repo = $1 AND id = ANY($2)", &[&repo, &edge_ids]).await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentops_graph::{prune_stale_nodes, upsert_node};

    /// Live against a real local Postgres — matching this rebuild's
    /// established discipline of verifying adapters against the real thing,
    /// not mocks. Reads `AGENTOPS_TEST_DATABASE_URL` if set (CI can point
    /// this anywhere), otherwise a local `pgvector/pgvector` container.
    /// Skips (not fails) when nothing is reachable, so this crate's test
    /// suite doesn't hard-require Docker/Postgres on every machine.
    fn test_store() -> Option<PostgresGraphStore> {
        let url = std::env::var("AGENTOPS_TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:test@localhost:5433/agentops_test".to_string());
        match PostgresGraphStore::connect(&url) {
            Ok(store) => Some(store),
            Err(e) => {
                eprintln!("skipping agentops-graph-pg live test: no Postgres reachable at {url} ({e})");
                None
            }
        }
    }

    macro_rules! require_store {
        () => {
            match test_store() {
                Some(s) => s,
                None => return,
            }
        };
    }

    fn node(repo: &str, kind: NodeKind, path: Option<&str>, name: Option<&str>) -> NewNode {
        NewNode { kind, repo: repo.to_string(), path: path.map(String::from), name: name.map(String::from), container: None, start_line: None, end_line: None, content: None }
    }

    #[test]
    fn upsert_inserts_then_updates_in_place_preserving_id() {
        let store = require_store!();
        let mut n = node("pg-repo-a", NodeKind::Symbol, Some("a.rs"), Some("foo"));
        n.content = Some("fn foo() {}".to_string());
        let id1 = upsert_node(&store, n.clone()).unwrap();

        n.content = Some("fn foo() { changed }".to_string());
        let id2 = upsert_node(&store, n).unwrap();

        assert_eq!(id1, id2, "re-upserting the same natural key must preserve the node's id");
        let got = store.get_node("pg-repo-a", id1).unwrap().unwrap();
        assert_eq!(got.content.as_deref(), Some("fn foo() { changed }"));

        store.delete_nodes("pg-repo-a", &[id1]).unwrap();
    }

    #[test]
    fn a_clone_shares_the_same_pool_not_a_fresh_one() {
        // Regression test for the actual production incident this Clone
        // impl fixes: confirms a cloned store sees writes made through the
        // original (same pool/connections), not an independent one -- the
        // property that lets `AppState` share one `PostgresGraphStore`
        // across concurrent requests instead of `connect()`-ing fresh per
        // request.
        let store = require_store!();
        let cloned = store.clone();

        let id = upsert_node(&store, node("pg-repo-clone", NodeKind::File, Some("a.rs"), None)).unwrap();
        let seen_via_clone = cloned.get_node("pg-repo-clone", id).unwrap();
        assert!(seen_via_clone.is_some(), "a clone must see writes made through the original store");

        store.delete_nodes("pg-repo-clone", &[id]).unwrap();
    }

    #[test]
    fn find_node_null_safe_comparison_does_not_cross_match_different_names() {
        let store = require_store!();
        let a = upsert_node(&store, node("pg-repo-b", NodeKind::File, Some("a.rs"), None)).unwrap();
        let b = upsert_node(&store, node("pg-repo-b", NodeKind::File, Some("b.rs"), None)).unwrap();
        assert_ne!(a, b);

        let found = store.find_node("pg-repo-b", NodeKind::File, Some("a.rs"), None, None).unwrap();
        assert_eq!(found.unwrap().path.as_deref(), Some("a.rs"));

        store.delete_nodes("pg-repo-b", &[a, b]).unwrap();
    }

    #[test]
    fn nodes_never_leak_across_repos() {
        let store = require_store!();
        let a = upsert_node(&store, node("pg-repo-c1", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();
        let b = upsert_node(&store, node("pg-repo-c2", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();

        assert!(store.get_node("pg-repo-c1", b).unwrap().is_none() || a != b);
        assert_eq!(store.nodes_by_kind("pg-repo-c1", NodeKind::Symbol).unwrap().iter().filter(|n| n.id == a).count(), 1);
        assert_eq!(store.nodes_by_kind("pg-repo-c1", NodeKind::Symbol).unwrap().iter().filter(|n| n.id == b).count(), 0);

        store.delete_nodes("pg-repo-c1", &[a]).unwrap();
        store.delete_nodes("pg-repo-c2", &[b]).unwrap();
    }

    #[test]
    fn references_edges_are_accepted_by_the_widened_relation_check_constraint() {
        // Regression test for the schema.sql migration this test file's
        // `connect()` re-applies on every run: `edges.relation`'s CHECK
        // constraint originally only admitted 'depends_on'/'documents'/
        // 'affects' -- without the widened constraint, this insert fails
        // with a real Postgres constraint-violation error, not a silent
        // no-op, so this is a meaningful live check that the migration
        // actually took effect against a real instance.
        let store = require_store!();
        let a = upsert_node(&store, node("pg-repo-refs", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();
        let b = upsert_node(&store, node("pg-repo-refs", NodeKind::Symbol, Some("a.rs"), Some("bar"))).unwrap();
        store.add_edge("pg-repo-refs", a, b, EdgeRelation::References).unwrap();

        let edges = store.edges_from("pg-repo-refs", a).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].relation, EdgeRelation::References, "must round-trip as References, not silently fall back to DependsOn");

        store.delete_nodes("pg-repo-refs", &[a, b]).unwrap();
    }

    #[test]
    fn edges_are_repo_scoped() {
        let store = require_store!();
        let a = upsert_node(&store, node("pg-repo-d", NodeKind::Symbol, Some("a.rs"), Some("foo"))).unwrap();
        let b = upsert_node(&store, node("pg-repo-d", NodeKind::Symbol, Some("a.rs"), Some("bar"))).unwrap();
        store.add_edge("pg-repo-d", a, b, EdgeRelation::DependsOn).unwrap();

        assert_eq!(store.edges_from("pg-repo-d", a).unwrap().len(), 1);
        assert_eq!(store.edges_from("pg-repo-other", a).unwrap().len(), 0, "a different repo scope must see zero edges even for the same node id");

        store.delete_nodes("pg-repo-d", &[a, b]).unwrap();
    }

    #[test]
    fn prune_stale_nodes_removes_only_untouched_nodes_in_scope() {
        let store = require_store!();
        let keep = upsert_node(&store, node("pg-repo-e", NodeKind::File, Some("keep.rs"), None)).unwrap();
        let stale = upsert_node(&store, node("pg-repo-e", NodeKind::File, Some("stale.rs"), None)).unwrap();

        let pruned = prune_stale_nodes(&store, "pg-repo-e", NodeKind::File, &[keep]).unwrap();
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].id, stale);
        assert!(store.get_node("pg-repo-e", keep).unwrap().is_some());
        assert!(store.get_node("pg-repo-e", stale).unwrap().is_none());

        store.delete_nodes("pg-repo-e", &[keep]).unwrap();
    }

    #[test]
    fn record_scan_derives_aggregate_counts_from_entries() {
        let store = require_store!();
        // scan_history has no delete path via the public GraphStore trait
        // (by design — it's an append-only log), and this test runs
        // against a real, persistent database rather than a fresh
        // in-memory one — a fixed repo name would accumulate scan rows
        // across repeated `cargo test` runs and break the `list_scans`
        // count below on the second run. A unique repo name per test
        // process makes this test repeatable regardless of prior runs.
        let repo = format!("pg-repo-f-{}", std::process::id());
        let entries = vec![
            NewScanHistoryEntry { node_id: 1, kind: NodeKind::File, path: Some("a.rs".into()), name: None, change: ScanChange::Added },
            NewScanHistoryEntry { node_id: 2, kind: NodeKind::Symbol, path: Some("a.rs".into()), name: Some("foo".into()), change: ScanChange::Added },
        ];
        let scan_id = store.record_scan(&repo, &entries).unwrap();

        let scan = store.latest_scan(&repo).unwrap().unwrap();
        assert_eq!(scan.id, scan_id);
        assert_eq!(scan.files_added, 1);
        assert_eq!(scan.symbols_added, 1);
        assert_eq!(store.scan_entries(scan_id).unwrap().len(), 2);
        assert_eq!(store.list_scans(&repo).unwrap().len(), 1);
    }

    fn unit_vec(dominant: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[dominant] = 1.0;
        v
    }

    #[test]
    fn search_similar_ranks_nearest_first_and_respects_repo_kind_scoping() {
        let store = require_store!();
        let close = upsert_node(&store, node("pg-repo-g", NodeKind::Symbol, Some("a.rs"), Some("close"))).unwrap();
        let far = upsert_node(&store, node("pg-repo-g", NodeKind::Symbol, Some("b.rs"), Some("far"))).unwrap();
        let other_kind = upsert_node(&store, node("pg-repo-g", NodeKind::Gotcha, None, Some("gotcha"))).unwrap();
        store.set_embedding("pg-repo-g", close, &unit_vec(0)).unwrap();
        store.set_embedding("pg-repo-g", far, &unit_vec(EMBEDDING_DIM - 1)).unwrap();
        store.set_embedding("pg-repo-g", other_kind, &unit_vec(0)).unwrap();

        let hits = store.search_similar("pg-repo-g", &unit_vec(0), 2, Some(NodeKind::Symbol)).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0.id, close, "the near vector must rank first: {hits:?}");

        store.delete_nodes("pg-repo-g", &[close, far, other_kind]).unwrap();
    }

    #[test]
    fn set_embedding_replaces_rather_than_duplicating() {
        let store = require_store!();
        let id = upsert_node(&store, node("pg-repo-h", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        store.set_embedding("pg-repo-h", id, &unit_vec(0)).unwrap();
        store.set_embedding("pg-repo-h", id, &unit_vec(1)).unwrap();

        let hits = store.search_similar("pg-repo-h", &unit_vec(1), 1, None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.id, id);

        store.delete_nodes("pg-repo-h", &[id]).unwrap();
    }

    #[test]
    fn set_curation_survives_a_subsequent_rescan() {
        let store = require_store!();
        let n = node("pg-repo-review", NodeKind::Gotcha, None, Some("g"));
        let id = upsert_node(&store, n.clone()).unwrap();
        let fresh = store.get_node("pg-repo-review", id).unwrap().unwrap();
        assert!(!fresh.curated);
        assert_eq!(fresh.prominence, NodeProminence::Full);

        store.set_curation("pg-repo-review", id, NodeProminence::Reduced, Some("only affects old Linux envs")).unwrap();
        let curated = store.get_node("pg-repo-review", id).unwrap().unwrap();
        assert_eq!(curated.prominence, NodeProminence::Reduced);
        assert_eq!(curated.curation_reason.as_deref(), Some("only affects old Linux envs"));

        // Same natural key upserted again, exactly what a rescan does.
        upsert_node(&store, n).unwrap();
        let after_rescan = store.get_node("pg-repo-review", id).unwrap().unwrap();
        assert_eq!(after_rescan.prominence, NodeProminence::Reduced, "a rescan must not silently reset a curated gotcha's prominence back to Full");

        store.delete_nodes("pg-repo-review", &[id]).unwrap();
    }

    /// Regression test for the exact reasoning `schema.sql` documents:
    /// unlike `SqliteGraphStore` (a separate `vec0` table that needs
    /// explicit orphan cleanup), `embedding` lives inline on `nodes` here,
    /// so a deleted node's embedding disappears for free.
    #[test]
    fn deleting_a_node_removes_its_embedding_for_free_via_the_row_itself() {
        let store = require_store!();
        let id = upsert_node(&store, node("pg-repo-i", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        store.set_embedding("pg-repo-i", id, &unit_vec(0)).unwrap();
        store.delete_nodes("pg-repo-i", &[id]).unwrap();

        let hits = store.search_similar("pg-repo-i", &unit_vec(0), 10, None).unwrap();
        assert!(hits.is_empty(), "found: {hits:?}");
    }

    #[test]
    fn set_embedding_rejects_a_node_outside_the_given_repo() {
        let store = require_store!();
        let id = upsert_node(&store, node("pg-repo-j", NodeKind::Symbol, Some("a.rs"), Some("sym"))).unwrap();
        assert!(store.set_embedding("pg-repo-other", id, &unit_vec(0)).is_err());
        store.delete_nodes("pg-repo-j", &[id]).unwrap();
    }

    #[test]
    fn upsert_nodes_batch_inserts_new_then_updates_the_same_natural_key_in_place() {
        let store = require_store!();
        let repo = "pg-repo-batch-upsert";
        let first = vec![node(repo, NodeKind::File, Some("a.py"), None)];
        let ids1 = store.upsert_nodes_batch(&first).unwrap();

        let mut second = node(repo, NodeKind::File, Some("a.py"), None);
        second.content = Some("changed".into());
        let ids2 = store.upsert_nodes_batch(&[second]).unwrap();

        assert_eq!(ids1, ids2, "same natural key must preserve the node's id, not create a second row");
        let got = store.get_node(repo, ids2[0]).unwrap().unwrap();
        assert_eq!(got.content.as_deref(), Some("changed"));

        store.delete_nodes(repo, &ids1).unwrap();
    }

    /// The highest-risk correctness detail this plan called out explicitly:
    /// a naive `UNNEST` + `=` join does *not* preserve `find_node`'s
    /// `IS NOT DISTINCT FROM` semantics for NULL path/name/container --
    /// two File nodes that both have no `name` must not collide with each
    /// other in a batch upsert.
    #[test]
    fn upsert_nodes_batch_does_not_cross_match_two_nodes_that_both_have_a_null_name() {
        let store = require_store!();
        let repo = "pg-repo-batch-null-safe";
        let nodes = vec![node(repo, NodeKind::File, Some("a.rs"), None), node(repo, NodeKind::File, Some("b.rs"), None)];
        let ids = store.upsert_nodes_batch(&nodes).unwrap();

        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "two distinct File nodes with no name must not collide into the same row");
        assert_eq!(store.get_node(repo, ids[0]).unwrap().unwrap().path.as_deref(), Some("a.rs"));
        assert_eq!(store.get_node(repo, ids[1]).unwrap().unwrap().path.as_deref(), Some("b.rs"));

        store.delete_nodes(repo, &ids).unwrap();
    }

    /// Regression test for the id-recovery join's ordinality-based ordering
    /// -- `upsert_nodes_batch` promises ids back in input order so a caller
    /// can `zip` the two slices; this is the property that promise actually
    /// depends on.
    #[test]
    fn upsert_nodes_batch_returns_ids_in_the_same_order_as_the_input() {
        let store = require_store!();
        let repo = "pg-repo-batch-order";
        let nodes: Vec<NewNode> = (0..5).map(|i| node(repo, NodeKind::File, Some(&format!("f{i}.rs")), None)).collect();
        let ids = store.upsert_nodes_batch(&nodes).unwrap();

        assert_eq!(ids.len(), 5);
        for (i, id) in ids.iter().enumerate() {
            let got = store.get_node(repo, *id).unwrap().unwrap();
            assert_eq!(got.path.as_deref(), Some(format!("f{i}.rs").as_str()), "id at position {i} must correspond to input position {i}");
        }

        store.delete_nodes(repo, &ids).unwrap();
    }

    /// Regression test for a real production incident: the scanner can
    /// legitimately emit two `Symbol` entries sharing the same natural key
    /// `(path, name, container)` within one file's symbol list, and the
    /// first version of this override sent both straight through to one
    /// `INSERT ... ON CONFLICT DO UPDATE` statement -- Postgres rejects
    /// that outright ("ON CONFLICT DO UPDATE command cannot affect row a
    /// second time"), which surfaced live as every scan of a real repo
    /// failing with "persisting scan: db error" the moment it hit a file
    /// with this shape. Confirms both that the call succeeds and that the
    /// id-lookup for the *duplicate* position still resolves correctly
    /// (same id as the position that actually won the upsert), preserving
    /// the "one id per input position" contract even though only one row
    /// existed in Postgres to answer both queries.
    #[test]
    fn upsert_nodes_batch_tolerates_two_entries_sharing_the_same_natural_key() {
        let store = require_store!();
        let repo = "pg-repo-batch-dup-key";
        let mut first = node(repo, NodeKind::Symbol, Some("a.rs"), Some("foo"));
        first.content = Some("v1".into());
        let mut second = node(repo, NodeKind::Symbol, Some("a.rs"), Some("foo"));
        second.content = Some("v2".into());

        let ids = store.upsert_nodes_batch(&[first, second]).unwrap();
        assert_eq!(ids.len(), 2, "one id per input position, even for the duplicate-keyed one");
        assert_eq!(ids[0], ids[1], "both positions share the same natural key, so they must resolve to the same underlying row");

        let got = store.get_node(repo, ids[0]).unwrap().unwrap();
        assert_eq!(got.content.as_deref(), Some("v2"), "last-wins, matching a sequential upsert_node loop's own semantics");

        store.delete_nodes(repo, &[ids[0]]).unwrap();
    }

    #[test]
    fn find_nodes_batch_does_not_cross_match_two_nodes_that_both_have_a_null_name() {
        let store = require_store!();
        let repo = "pg-repo-batch-find-null-safe";
        let ids = store.upsert_nodes_batch(&[node(repo, NodeKind::File, Some("a.rs"), None), node(repo, NodeKind::File, Some("b.rs"), None)]).unwrap();

        let found = store.find_nodes_batch(repo, &[(NodeKind::File, Some("a.rs"), None, None), (NodeKind::File, Some("b.rs"), None, None)]).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found.get(&(NodeKind::File, Some("a.rs".to_string()), None, None)).map(|n| n.path.as_deref()), Some(Some("a.rs")));
        assert_eq!(found.get(&(NodeKind::File, Some("b.rs".to_string()), None, None)).map(|n| n.path.as_deref()), Some(Some("b.rs")));

        store.delete_nodes(repo, &ids).unwrap();
    }

    #[test]
    fn set_embeddings_batch_round_trips_every_vector_to_its_own_node() {
        let store = require_store!();
        let repo = "pg-repo-batch-embed";
        let ids = store.upsert_nodes_batch(&[node(repo, NodeKind::Symbol, Some("a.rs"), Some("a")), node(repo, NodeKind::Symbol, Some("b.rs"), Some("b"))]).unwrap();

        store.set_embeddings_batch(repo, &[(ids[0], unit_vec(0)), (ids[1], unit_vec(1))]).unwrap();

        let hits = store.search_similar(repo, &unit_vec(0), 1, None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.id, ids[0], "the node embedded with unit_vec(0) must be the nearest match for that same query");

        store.delete_nodes(repo, &ids).unwrap();
    }

    #[test]
    fn add_edges_batch_creates_every_edge_in_one_call() {
        let store = require_store!();
        let repo = "pg-repo-batch-edges";
        let a = test_symbol_pg(&store, repo, "a");
        let b = test_symbol_pg(&store, repo, "b");
        let c = test_symbol_pg(&store, repo, "c");

        store.add_edges_batch(repo, &[(a, b, EdgeRelation::DependsOn), (a, c, EdgeRelation::References)]).unwrap();

        let edges = store.edges_from(repo, a).unwrap();
        assert_eq!(edges.len(), 2);

        store.delete_nodes(repo, &[a, b, c]).unwrap();
    }

    #[test]
    fn edges_from_batch_includes_an_empty_vec_for_a_src_id_with_no_edges() {
        let store = require_store!();
        let repo = "pg-repo-batch-edges-from";
        let a = test_symbol_pg(&store, repo, "a");
        let b = test_symbol_pg(&store, repo, "b");
        let c = test_symbol_pg(&store, repo, "c");
        store.add_edge(repo, a, b, EdgeRelation::DependsOn).unwrap();

        let result = store.edges_from_batch(repo, &[a, c]).unwrap();
        assert_eq!(result.get(&a).map(Vec::len), Some(1));
        assert_eq!(result.get(&c).map(Vec::len), Some(0), "a src_id with no edges must still appear, mapped to an empty Vec");

        store.delete_nodes(repo, &[a, b, c]).unwrap();
    }

    #[test]
    fn reinforce_edges_batch_bumps_every_edges_weight_and_respects_bump_confirmed_at() {
        let store = require_store!();
        let repo = "pg-repo-batch-reinforce";
        let a = test_symbol_pg(&store, repo, "a");
        let b = test_symbol_pg(&store, repo, "b");
        let c = test_symbol_pg(&store, repo, "c");
        let e1 = store.add_edge(repo, a, b, EdgeRelation::Affects).unwrap();
        let e2 = store.add_edge(repo, a, c, EdgeRelation::Affects).unwrap();

        store.reinforce_edges_batch(repo, &[e1, e2], false).unwrap();
        let edges = store.edges_from(repo, a).unwrap();
        assert!(edges.iter().all(|e| e.weight > 1.0), "{edges:?}");

        store.delete_nodes(repo, &[a, b, c]).unwrap();
    }

    #[test]
    fn delete_edges_batch_removes_every_listed_edge_but_leaves_others() {
        let store = require_store!();
        let repo = "pg-repo-batch-delete-edges";
        let a = test_symbol_pg(&store, repo, "a");
        let b = test_symbol_pg(&store, repo, "b");
        let c = test_symbol_pg(&store, repo, "c");
        let e1 = store.add_edge(repo, a, b, EdgeRelation::DependsOn).unwrap();
        let e2 = store.add_edge(repo, a, c, EdgeRelation::DependsOn).unwrap();

        store.delete_edges_batch(repo, &[e1]).unwrap();
        let remaining = store.edges_from(repo, a).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, e2);

        store.delete_nodes(repo, &[a, b, c]).unwrap();
    }

    #[test]
    fn snapshot_node_versions_batch_closes_the_open_row_and_opens_a_new_one_for_every_node() {
        let store = require_store!();
        let repo = "pg-repo-batch-versions";
        let a = test_symbol_pg(&store, repo, "a");
        let b = test_symbol_pg(&store, repo, "b");
        store.snapshot_node_version(a, Some("v1"), Some(1), Some(2)).unwrap();
        store.snapshot_node_version(b, Some("v1"), Some(1), Some(2)).unwrap();

        store.snapshot_node_versions_batch(repo, &[(a, Some("v2"), Some(1), Some(3)), (b, Some("v2"), Some(1), Some(3))]).unwrap();

        for id in [a, b] {
            let history = store.node_history(id).unwrap();
            assert_eq!(history.len(), 2, "{history:?}");
            assert_eq!(history[0].content.as_deref(), Some("v2"), "the most recent version must be the batch's new one");
            assert!(history[1].valid_until.is_some(), "the original version must have been closed, not left open");
        }

        store.delete_nodes(repo, &[a, b]).unwrap();
    }

    fn test_symbol_pg(store: &PostgresGraphStore, repo: &str, name: &str) -> i64 {
        upsert_node(store, node(repo, NodeKind::Symbol, Some(&format!("{name}.rs")), Some(name))).unwrap()
    }
}
