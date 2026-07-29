//! Postgres-backed `GraphStore` — the heavy tier. Same trait, same node/edge
//! shape as the light tier's `SqliteGraphStore` (see the plan's "the same
//! graph, replicated into scalable stores"); this crate is the part that's
//! commercially licensed, not the shape of the data itself.
//!
//! `GraphStore`'s methods are synchronous (`&self`, no `async`), but
//! `tokio-postgres`/`deadpool-postgres` are async-only. `PostgresGraphStore`
//! owns a small, dedicated multi-thread Tokio runtime and calls
//! `runtime.block_on(...)` inside every method — deliberately its OWN
//! runtime, not `Handle::current()`, so this is safe to call from a plain
//! synchronous context (a test, a CLI command) *and* from within another
//! async runtime (e.g. an `agentops-api` handler) without the "block_on
//! inside a running runtime" deadlock that `Handle::current().block_on()`
//! risks — two separate runtimes never contend with each other.

use anyhow::{Context, Result};
use deadpool_postgres::{Config, Pool, Runtime as DeadpoolRuntime};
use agentops_graph::{EdgeRelation, GraphStore, NewNode, Node, NodeKind, Edge};
use tokio_postgres::{NoTls, Row};

pub struct PostgresGraphStore {
    pool: Pool,
    runtime: tokio::runtime::Runtime,
}

impl PostgresGraphStore {
    /// Connects using a standard Postgres URL
    /// (`postgres://user:pass@host:port/dbname`). Assumes the schema in
    /// `agentops-heavy/docker/postgres-init/001_schema.sql` has already been
    /// applied (via the Docker image's init hook, or manually).
    pub fn connect(database_url: &str) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("building the dedicated Postgres runtime")?;

        let mut cfg = Config::new();
        cfg.url = Some(database_url.to_string());
        let pool = cfg
            .create_pool(Some(DeadpoolRuntime::Tokio1), NoTls)
            .context("creating the Postgres connection pool")?;

        // Fail fast on a bad connection string / unreachable server, rather
        // than deferring the first error to whatever the first real query is.
        runtime.block_on(async {
            let client = pool.get().await.context("getting a pooled connection to verify connectivity")?;
            client.simple_query("SELECT 1").await.context("verifying Postgres connectivity")?;
            Ok::<_, anyhow::Error>(())
        })?;

        Ok(Self { pool, runtime })
    }

    fn row_to_node(row: &Row) -> Result<Node> {
        let kind_str: String = row.get("kind");
        Ok(Node {
            id: row.get("id"),
            kind: NodeKind::from_str(&kind_str)?,
            repo: row.get("repo"),
            path: row.get("path"),
            name: row.get("name"),
            start_line: row.get("start_line"),
            end_line: row.get("end_line"),
            content: row.get("content"),
        })
    }

    fn row_to_edge(row: &Row) -> Result<Edge> {
        let relation_str: String = row.get("relation");
        Ok(Edge {
            id: row.get("id"),
            src_id: row.get("src_id"),
            dst_id: row.get("dst_id"),
            relation: EdgeRelation::from_str(&relation_str)?,
        })
    }
}

impl GraphStore for PostgresGraphStore {
    fn add_node(&self, node: NewNode) -> Result<i64> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_one(
                    "INSERT INTO nodes (kind, repo, path, name, start_line, end_line, content)
                     VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
                    &[
                        &node.kind.as_str(),
                        &node.repo,
                        &node.path,
                        &node.name,
                        &node.start_line,
                        &node.end_line,
                        &node.content,
                    ],
                )
                .await?;
            Ok::<i64, anyhow::Error>(row.get("id"))
        })
    }

    fn add_edge(&self, src_id: i64, dst_id: i64, relation: EdgeRelation) -> Result<i64> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            let row = client
                .query_one(
                    "INSERT INTO edges (src_id, dst_id, relation) VALUES ($1, $2, $3) RETURNING id",
                    &[&src_id, &dst_id, &relation.as_str()],
                )
                .await?;
            Ok::<i64, anyhow::Error>(row.get("id"))
        })
    }

    fn get_node(&self, id: i64) -> Result<Option<Node>> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            let row = client.query_opt("SELECT * FROM nodes WHERE id = $1", &[&id]).await?;
            row.map(|r| Self::row_to_node(&r)).transpose()
        })
    }

    fn nodes_by_kind(&self, kind: NodeKind) -> Result<Vec<Node>> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT * FROM nodes WHERE kind = $1", &[&kind.as_str()]).await?;
            rows.iter().map(Self::row_to_node).collect()
        })
    }

    fn edges_from(&self, src_id: i64) -> Result<Vec<Edge>> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT * FROM edges WHERE src_id = $1", &[&src_id]).await?;
            rows.iter().map(Self::row_to_edge).collect()
        })
    }

    fn edges_to(&self, dst_id: i64) -> Result<Vec<Edge>> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT * FROM edges WHERE dst_id = $1", &[&dst_id]).await?;
            rows.iter().map(Self::row_to_edge).collect()
        })
    }

    fn all_nodes(&self) -> Result<Vec<Node>> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT * FROM nodes", &[]).await?;
            rows.iter().map(Self::row_to_node).collect()
        })
    }

    fn all_edges(&self) -> Result<Vec<Edge>> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            let rows = client.query("SELECT * FROM edges", &[]).await?;
            rows.iter().map(Self::row_to_edge).collect()
        })
    }

    fn find_node(&self, repo: &str, kind: NodeKind, path: Option<&str>, name: Option<&str>) -> Result<Option<Node>> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            // `IS NOT DISTINCT FROM` (not `=`) so a NULL path/name matches
            // NULL rather than never matching at all.
            let row = client
                .query_opt(
                    "SELECT * FROM nodes WHERE repo = $1 AND kind = $2 AND path IS NOT DISTINCT FROM $3 AND name IS NOT DISTINCT FROM $4",
                    &[&repo, &kind.as_str(), &path, &name],
                )
                .await?;
            row.map(|r| Self::row_to_node(&r)).transpose()
        })
    }

    fn update_node(&self, id: i64, start_line: Option<i64>, end_line: Option<i64>, content: Option<String>) -> Result<()> {
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            client
                .execute("UPDATE nodes SET start_line = $1, end_line = $2, content = $3 WHERE id = $4", &[&start_line, &end_line, &content, &id])
                .await?;
            Ok::<(), anyhow::Error>(())
        })
    }

    fn delete_nodes(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.runtime.block_on(async {
            let client = self.pool.get().await?;
            client.execute("DELETE FROM edges WHERE src_id = ANY($1) OR dst_id = ANY($1)", &[&ids]).await?;
            client.execute("DELETE FROM nodes WHERE id = ANY($1)", &[&ids]).await?;
            Ok::<(), anyhow::Error>(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests hit a REAL Postgres instance — the docker-compose stack in
    // agentops-heavy/docker/. Set AGENTOPS_TEST_DATABASE_URL to run them;
    // they're skipped (not failed) otherwise, since not every environment
    // running `cargo test` has Docker available (matching this project's
    // established pattern of verifying against real services where
    // possible, without making that a hard requirement to build at all).

    fn test_store() -> Option<PostgresGraphStore> {
        let url = std::env::var("AGENTOPS_TEST_DATABASE_URL").ok()?;
        Some(PostgresGraphStore::connect(&url).expect("connect to test Postgres"))
    }

    fn cleanup(store: &PostgresGraphStore, repo: &str) {
        // Best-effort test isolation: delete anything this test run created,
        // scoped by a unique repo name per test, so tests don't interfere
        // with each other or leave permanent cruft in a shared dev database.
        let _ = store.runtime.block_on(async {
            let client = store.pool.get().await?;
            client.execute("DELETE FROM nodes WHERE repo = $1", &[&repo]).await?;
            Ok::<(), anyhow::Error>(())
        });
    }

    #[test]
    fn insert_and_query_node() {
        let Some(store) = test_store() else { return };
        let repo = "test-insert-and-query-node";
        cleanup(&store, repo);

        let id = store
            .add_node(NewNode {
                kind: NodeKind::Symbol,
                repo: repo.into(),
                path: Some("src/lib.rs".into()),
                name: Some("do_thing".into()),
                start_line: Some(1),
                end_line: Some(10),
                content: Some("fn do_thing() {}".into()),
            })
            .unwrap();

        let node = store.get_node(id).unwrap().expect("node exists");
        assert_eq!(node.name.as_deref(), Some("do_thing"));
        assert_eq!(node.kind, NodeKind::Symbol);

        cleanup(&store, repo);
    }

    #[test]
    fn gotcha_node_connects_to_symbol_via_affects_edge() {
        let Some(store) = test_store() else { return };
        let repo = "test-gotcha-affects-edge";
        cleanup(&store, repo);

        let symbol_id = store
            .add_node(NewNode {
                kind: NodeKind::Symbol,
                repo: repo.into(),
                path: Some("src/auth.rs".into()),
                name: Some("verify_token".into()),
                start_line: Some(5),
                end_line: Some(20),
                content: Some("fn verify_token() {}".into()),
            })
            .unwrap();

        let gotcha_id = store
            .add_node(NewNode {
                kind: NodeKind::Gotcha,
                repo: repo.into(),
                path: None,
                name: Some("token-expiry-off-by-one".into()),
                start_line: None,
                end_line: None,
                content: Some("Token expiry check was off by one day.".into()),
            })
            .unwrap();

        store.add_edge(gotcha_id, symbol_id, EdgeRelation::Affects).unwrap();

        let incoming = store.edges_to(symbol_id).unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].src_id, gotcha_id);
        assert_eq!(incoming[0].relation, EdgeRelation::Affects);

        cleanup(&store, repo);
    }

    #[test]
    fn connect_fails_fast_on_a_bad_url_instead_of_deferring_the_error() {
        let result = PostgresGraphStore::connect("postgres://nope:nope@127.0.0.1:1/nonexistent");
        assert!(result.is_err());
    }

    fn symbol_node(repo: &str, path: &str, name: &str, content: &str) -> NewNode {
        NewNode { kind: NodeKind::Symbol, repo: repo.into(), path: Some(path.into()), name: Some(name.into()), start_line: Some(1), end_line: Some(2), content: Some(content.into()) }
    }

    #[test]
    fn upserting_the_same_symbol_twice_updates_in_place_instead_of_duplicating() {
        let Some(store) = test_store() else { return };
        let repo = "test-upsert-no-duplicate";
        cleanup(&store, repo);

        let id1 = agentops_graph::upsert_node(&store, symbol_node(repo, "src/lib.rs", "do_thing", "v1")).unwrap();
        let id2 = agentops_graph::upsert_node(&store, symbol_node(repo, "src/lib.rs", "do_thing", "v2")).unwrap();

        assert_eq!(id1, id2, "rescanning the same symbol must reuse its id, not create a new one");
        let symbols: Vec<_> = store.nodes_by_kind(NodeKind::Symbol).unwrap().into_iter().filter(|n| n.repo == repo).collect();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].content.as_deref(), Some("v2"));

        cleanup(&store, repo);
    }

    #[test]
    fn upsert_preserves_id_so_existing_gotcha_edges_survive_a_rescan() {
        let Some(store) = test_store() else { return };
        let repo = "test-upsert-preserves-edges";
        cleanup(&store, repo);

        let symbol_id = agentops_graph::upsert_node(&store, symbol_node(repo, "src/auth.rs", "verify_token", "v1")).unwrap();
        let gotcha_id = store
            .add_node(NewNode { kind: NodeKind::Gotcha, repo: repo.into(), path: None, name: Some("g".into()), start_line: None, end_line: None, content: Some("text".into()) })
            .unwrap();
        store.add_edge(gotcha_id, symbol_id, EdgeRelation::Affects).unwrap();

        let rescanned_id = agentops_graph::upsert_node(&store, symbol_node(repo, "src/auth.rs", "verify_token", "v1")).unwrap();
        assert_eq!(rescanned_id, symbol_id);

        let incoming = store.edges_to(symbol_id).unwrap();
        assert_eq!(incoming.len(), 1, "the gotcha's edge must still resolve after a rescan");

        cleanup(&store, repo);
    }

    #[test]
    fn prune_stale_nodes_removes_symbols_missing_from_the_latest_scan_and_their_edges() {
        let Some(store) = test_store() else { return };
        let repo = "test-prune-stale";
        cleanup(&store, repo);

        let kept_id = agentops_graph::upsert_node(&store, symbol_node(repo, "src/lib.rs", "kept_fn", "..")).unwrap();
        let removed_id = agentops_graph::upsert_node(&store, symbol_node(repo, "src/lib.rs", "removed_fn", "..")).unwrap();
        let gotcha_id = store
            .add_node(NewNode { kind: NodeKind::Gotcha, repo: repo.into(), path: None, name: Some("g".into()), start_line: None, end_line: None, content: Some("text".into()) })
            .unwrap();
        store.add_edge(gotcha_id, removed_id, EdgeRelation::Affects).unwrap();

        let pruned = agentops_graph::prune_stale_nodes(&store, repo, NodeKind::Symbol, &[kept_id]).unwrap();

        assert_eq!(pruned, 1);
        assert!(store.get_node(kept_id).unwrap().is_some());
        assert!(store.get_node(removed_id).unwrap().is_none());
        assert!(store.edges_to(removed_id).unwrap().is_empty());

        cleanup(&store, repo);
    }
}
