//! `PostgresGraphStore`: an optional `GraphStore` adapter for a shared,
//! multi-repo Postgres database — the "hypothetical future shared/
//! multi-tenant adapter" `agentops-graph`'s own module doc comment already
//! named as the reason repo-scoping is required on every trait method.
//! Part of `agentops-core`, not a paid heavy-tier gate — the same "no
//! tiering" decision docbrain made for itself, extended to codebrain (see
//! the plan). Schema: `schema.sql` in this crate's root, a structural
//! mirror of `SqliteGraphStore`'s current schema, not a port of `main`'s
//! stale `agentops-heavy` one.
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

use agentops_embeddings::EMBEDDING_DIM;
use agentops_graph::{rank_notes_by_weight, Edge, EdgeRelation, GraphStore, NewNode, NewScanHistoryEntry, NewTask, Node, NodeKind, NodeProminence, NodeVersion, RepoState, ScanChange, ScanHistory, ScanHistoryEntry, SessionEvent, Task, TaskLink, TaskStatus};
use anyhow::Result;

const SCHEMA: &str = include_str!("../schema.sql");

pub struct PostgresGraphStore {
    // Kept alive for the store's whole lifetime, not just at construction —
    // `deadpool-postgres`'s `Runtime::Tokio1`-mode pool needs an active
    // Tokio context for background connection recycling the entire time
    // it's in use. Dropping this right after building the pool would panic
    // on the first query issued afterward.
    rt: tokio::runtime::Runtime,
    pool: deadpool_postgres::Pool,
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
            let pool = cfg.create_pool(Some(deadpool_postgres::Runtime::Tokio1), tokio_postgres::NoTls)?;
            let client = pool.get().await?;
            client.batch_execute(SCHEMA).await?;
            Ok::<deadpool_postgres::Pool, anyhow::Error>(pool)
        })?;
        Ok(Self { rt, pool })
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
    SessionEvent { id: row.get("id"), repo: row.get("repo"), session_id: row.get("session_id"), tool_name: row.get("tool_name"), description: row.get("description"), created_at: row.get("created_at") }
}

// Same `::text` cast reasoning as EDGES_COLUMNS.
const SESSION_EVENTS_COLUMNS: &str = "id, repo, session_id, tool_name, description, created_at::text AS created_at";

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
            let row = client.query_opt(&sql, &[&node_id, &timestamp]).await?;
            Ok(row.as_ref().map(row_to_node_version))
        })
    }

    fn record_session_event(&self, repo: &str, session_id: &str, tool_name: &str, description: &str) -> Result<i64> {
        self.rt.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_one(
                    "INSERT INTO session_events (repo, session_id, tool_name, description) VALUES ($1, $2, $3, $4) RETURNING id",
                    &[&repo, &session_id, &tool_name, &description],
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
}
