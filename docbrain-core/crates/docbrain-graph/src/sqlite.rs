//! `SqliteDocbrainStore`: the SQLite adapter implementing the `DocbrainStore`
//! port. Vector search is real KNN via `sqlite-vec`'s `vec0` virtual table on
//! the same connection/file as everything else — no separate Qdrant/Postgres
//! service required (see the plan's "one system, no tiering" design).

use std::path::Path;
use std::sync::Once;

use anyhow::{Context, Result};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::{ChangelogEntry, DocEdge, DocNode, DocbrainStore, EdgeRelation, Job, JobStatus, Library, NewDocNode, NodeKind, SearchHit, UpsertOutcome};

/// Embedding dimension for docbrain's default model (BGE-small-en-v1.5,
/// chosen in `docbrain-ingest::embed` for a light local footprint). The
/// `vec0` virtual table's column width is fixed at creation time, so this
/// has to be a compile-time constant shared between the two crates rather
/// than inferred from whatever vector happens to be inserted first.
pub const EMBEDDING_DIM: usize = 384;

static INIT_VEC_EXTENSION: Once = Once::new();

/// `sqlite-vec` is loaded via `sqlite3_auto_extension`, which registers it
/// for every connection opened in this process from that point on — must
/// run before any `Connection::open*` call, and only once per process
/// (calling it twice would register the init function twice).
fn ensure_vec_extension_registered() {
    INIT_VEC_EXTENSION.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::ffi::c_int,
        >(sqlite_vec::sqlite3_vec_init as *const ())));
    });
}

const MIGRATIONS: &str = "
    CREATE TABLE IF NOT EXISTS libraries (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        slug        TEXT NOT NULL UNIQUE,
        name        TEXT NOT NULL,
        github_repo TEXT,
        docs_url    TEXT
    );

    CREATE TABLE IF NOT EXISTS doc_snapshots (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        library_id  INTEGER NOT NULL REFERENCES libraries(id),
        version     TEXT NOT NULL,
        scraped_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );

    -- content_hash dedup key is scoped to (library_id, version): re-scraping
    -- the same version only inserts nodes whose hash is new, closing the
    -- confirmed 'main'-branch gap where every re-scrape duplicated every node.
    CREATE TABLE IF NOT EXISTS doc_nodes (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        library_id    INTEGER NOT NULL REFERENCES libraries(id),
        version       TEXT NOT NULL,
        topic         TEXT NOT NULL,
        content       TEXT NOT NULL,
        content_hash  TEXT NOT NULL,
        token_count   INTEGER NOT NULL,
        kind          TEXT NOT NULL DEFAULT 'prose'
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_doc_nodes_dedup ON doc_nodes(library_id, version, content_hash);
    CREATE INDEX IF NOT EXISTS idx_doc_nodes_lib_ver ON doc_nodes(library_id, version);

    CREATE TABLE IF NOT EXISTS doc_edges (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        from_node_id INTEGER NOT NULL REFERENCES doc_nodes(id),
        to_node_id   INTEGER NOT NULL REFERENCES doc_nodes(id),
        relation     TEXT NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_doc_edges_unique ON doc_edges(from_node_id, to_node_id, relation);
    CREATE INDEX IF NOT EXISTS idx_doc_edges_from ON doc_edges(from_node_id);
    CREATE INDEX IF NOT EXISTS idx_doc_edges_to ON doc_edges(to_node_id);

    -- Idempotency key fixing 'main''s real bug: a plain INSERT with no
    -- unique constraint meant re-running sync duplicated every row.
    CREATE TABLE IF NOT EXISTS changelog_versions (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        library_id   INTEGER NOT NULL REFERENCES libraries(id),
        from_version TEXT NOT NULL,
        to_version   TEXT NOT NULL,
        entry        TEXT NOT NULL,
        breaking     INTEGER NOT NULL DEFAULT 0
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_changelog_dedup ON changelog_versions(library_id, from_version, to_version);

    CREATE TABLE IF NOT EXISTS jobs (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        job_type    TEXT NOT NULL,
        slug        TEXT NOT NULL,
        status      TEXT NOT NULL DEFAULT 'running',
        message     TEXT,
        created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        updated_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
";

fn vec_table_migration() -> String {
    format!("CREATE VIRTUAL TABLE IF NOT EXISTS doc_node_vectors USING vec0(node_id INTEGER PRIMARY KEY, embedding FLOAT[{EMBEDDING_DIM}]);")
}

pub struct SqliteDocbrainStore {
    conn: Connection,
}

impl SqliteDocbrainStore {
    pub fn open(path: &Path) -> Result<Self> {
        ensure_vec_extension_registered();
        let conn = agentops_sqlite_support::open(path, MIGRATIONS)?;
        conn.execute_batch(&vec_table_migration()).context("creating doc_node_vectors virtual table")?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        ensure_vec_extension_registered();
        let conn = agentops_sqlite_support::open_in_memory(MIGRATIONS)?;
        conn.execute_batch(&vec_table_migration()).context("creating doc_node_vectors virtual table")?;
        Ok(Self { conn })
    }

    fn library_id(&self, slug: &str) -> Result<Option<i64>> {
        self.conn
            .query_row("SELECT id FROM libraries WHERE slug = ?1", [slug], |r| r.get(0))
            .map(Some)
            .or_else(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) { Ok(None) } else { Err(e.into()) })
    }

    fn authorized_library_id(&self, slug: &str) -> Result<i64> {
        self.library_id(slug)?.ok_or_else(|| anyhow::anyhow!("no library '{slug}' registered"))
    }

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
        let id: i64 = row.get("id")?;
        let (doc_snapshots, changelog_versions, versions, total_nodes) = self.stats_for(id)?;
        Ok(Library {
            id,
            slug: row.get("slug")?,
            name: row.get("name")?,
            github_repo: row.get("github_repo")?,
            docs_url: row.get("docs_url")?,
            doc_snapshots,
            changelog_versions,
            versions,
            total_nodes,
        })
    }

    fn row_to_node(row: &rusqlite::Row) -> rusqlite::Result<DocNode> {
        let kind: String = row.get("kind")?;
        Ok(DocNode {
            id: row.get("id")?,
            library_id: row.get("library_id")?,
            version: row.get("version")?,
            topic: row.get("topic")?,
            content: row.get("content")?,
            content_hash: row.get("content_hash")?,
            token_count: row.get("token_count")?,
            kind: NodeKind::from_db_str(&kind),
        })
    }
}

fn embedding_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

impl DocbrainStore for SqliteDocbrainStore {
    fn add_library(&self, slug: &str, name: &str, github_repo: Option<&str>, docs_url: Option<&str>) -> Result<i64> {
        agentops_sqlite_support::upsert(
            &self.conn,
            "libraries",
            &["slug", "name", "github_repo", "docs_url"],
            &["slug"],
            &[&slug, &name, &github_repo, &docs_url],
        )
    }

    fn get_library(&self, slug: &str) -> Result<Option<Library>> {
        let mut stmt = self.conn.prepare("SELECT * FROM libraries WHERE slug = ?1")?;
        let mut rows = stmt.query_map([slug], |row| self.row_to_library(row))?;
        Ok(rows.next().transpose()?)
    }

    fn list_libraries(&self) -> Result<Vec<Library>> {
        let mut stmt = self.conn.prepare("SELECT * FROM libraries ORDER BY slug")?;
        let rows = stmt.query_map([], |row| self.row_to_library(row))?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn add_doc_snapshot(&self, slug: &str, version: &str) -> Result<i64> {
        let library_id = self.authorized_library_id(slug)?;
        self.conn.execute("INSERT INTO doc_snapshots (library_id, version) VALUES (?1, ?2)", rusqlite::params![library_id, version])?;
        Ok(self.conn.last_insert_rowid())
    }

    fn upsert_doc_node(&self, slug: &str, node: NewDocNode) -> Result<UpsertOutcome> {
        let library_id = self.authorized_library_id(slug)?;

        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM doc_nodes WHERE library_id = ?1 AND version = ?2 AND content_hash = ?3",
                rusqlite::params![library_id, node.version, node.content_hash],
                |r| r.get(0),
            )
            .ok();
        if let Some(id) = existing {
            return Ok(UpsertOutcome::AlreadyExists(id));
        }

        self.conn.execute(
            "INSERT INTO doc_nodes (library_id, version, topic, content, content_hash, token_count, kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![library_id, node.version, node.topic, node.content, node.content_hash, node.token_count, node.kind.as_db_str()],
        )?;
        let id = self.conn.last_insert_rowid();

        if let Some(embedding) = node.embedding {
            anyhow::ensure!(embedding.len() == EMBEDDING_DIM, "embedding has {} dims, expected {EMBEDDING_DIM}", embedding.len());
            self.conn.execute(
                "INSERT INTO doc_node_vectors (node_id, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, embedding_to_bytes(embedding)],
            )?;
        }

        Ok(UpsertOutcome::Inserted(id))
    }

    fn get_node(&self, node_id: i64) -> Result<Option<DocNode>> {
        let mut stmt = self.conn.prepare("SELECT * FROM doc_nodes WHERE id = ?1")?;
        let mut rows = stmt.query_map([node_id], Self::row_to_node)?;
        Ok(rows.next().transpose()?)
    }

    fn get_doc_nodes(&self, slug: &str, version: &str) -> Result<Vec<DocNode>> {
        let library_id = self.authorized_library_id(slug)?;
        let mut stmt = self.conn.prepare("SELECT * FROM doc_nodes WHERE library_id = ?1 AND version = ?2")?;
        let rows = stmt.query_map(rusqlite::params![library_id, version], Self::row_to_node)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn all_doc_nodes(&self, slug: &str) -> Result<Vec<DocNode>> {
        let library_id = self.authorized_library_id(slug)?;
        let mut stmt = self.conn.prepare("SELECT * FROM doc_nodes WHERE library_id = ?1")?;
        let rows = stmt.query_map([library_id], Self::row_to_node)?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn list_doc_versions(&self, slug: &str) -> Result<Vec<String>> {
        let library_id = self.authorized_library_id(slug)?;
        let mut stmt = self.conn.prepare("SELECT DISTINCT version FROM doc_snapshots WHERE library_id = ?1 ORDER BY version")?;
        let rows = stmt.query_map([library_id], |row| row.get::<_, String>(0))?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn add_doc_edge(&self, from_node: i64, to_node: i64, relation: EdgeRelation) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO doc_edges (from_node_id, to_node_id, relation) VALUES (?1, ?2, ?3)",
            rusqlite::params![from_node, to_node, relation.as_db_str()],
        )?;
        Ok(())
    }

    fn node_edges(&self, node_id: i64) -> Result<Vec<DocEdge>> {
        let mut stmt = self.conn.prepare("SELECT * FROM doc_edges WHERE from_node_id = ?1 OR to_node_id = ?1")?;
        let rows = stmt.query_map([node_id], |row| {
            let relation: String = row.get("relation")?;
            Ok(DocEdge {
                id: row.get("id")?,
                from_node: row.get("from_node_id")?,
                to_node: row.get("to_node_id")?,
                relation: EdgeRelation::from_db_str(&relation),
            })
        })?;
        rows.map(|r| r.map_err(anyhow::Error::from)).collect()
    }

    fn search_similar(&self, embedding: &[f32], top_k: usize, slug: Option<&str>, exclude_kind: Option<NodeKind>) -> Result<Vec<SearchHit>> {
        anyhow::ensure!(embedding.len() == EMBEDDING_DIM, "query embedding has {} dims, expected {EMBEDDING_DIM}", embedding.len());
        let library_id = slug.map(|s| self.authorized_library_id(s)).transpose()?;

        // Retrieve a wider candidate pool than top_k, then filter by library
        // and kind in Rust — vec0's own filtering support varies by version,
        // and this scan is cheap at the node counts a self-hosted docbrain
        // instance actually has.
        let fetch_k = (top_k * 8).max(50) as i64;
        let mut stmt = self.conn.prepare("SELECT node_id, distance FROM doc_node_vectors WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance")?;
        let rows = stmt.query_map(rusqlite::params![embedding_to_bytes(embedding), fetch_k], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;

        let mut hits = Vec::new();
        for row in rows {
            let (node_id, distance) = row?;
            let Some(node) = self.get_node(node_id)? else { continue };
            if library_id.is_some_and(|lid| node.library_id != lid) {
                continue;
            }
            if exclude_kind.is_some_and(|k| node.kind == k) {
                continue;
            }
            hits.push(SearchHit { node, distance: distance as f32 });
            if hits.len() >= top_k {
                break;
            }
        }
        Ok(hits)
    }

    fn add_changelog_entry(&self, slug: &str, from_version: &str, to_version: &str, entry: &str, breaking: bool) -> Result<i64> {
        let library_id = self.authorized_library_id(slug)?;
        agentops_sqlite_support::upsert(
            &self.conn,
            "changelog_versions",
            &["library_id", "from_version", "to_version", "entry", "breaking"],
            &["library_id", "from_version", "to_version"],
            &[&library_id, &from_version, &to_version, &entry, &(breaking as i64)],
        )
    }

    fn get_changelog(&self, slug: &str, from_version: &str, to_version: &str) -> Result<Vec<ChangelogEntry>> {
        let library_id = self.authorized_library_id(slug)?;
        let mut stmt =
            self.conn.prepare("SELECT * FROM changelog_versions WHERE library_id = ?1 AND from_version = ?2 AND to_version = ?3")?;
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

    fn create_job(&self, job_type: &str, slug: &str) -> Result<i64> {
        self.conn.execute("INSERT INTO jobs (job_type, slug, status) VALUES (?1, ?2, 'running')", rusqlite::params![job_type, slug])?;
        Ok(self.conn.last_insert_rowid())
    }

    fn complete_job(&self, id: i64, status: JobStatus, message: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs SET status = ?1, message = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
            rusqlite::params![status.as_db_str(), message, id],
        )?;
        Ok(())
    }

    fn get_job(&self, id: i64) -> Result<Option<Job>> {
        let mut stmt = self.conn.prepare("SELECT * FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], |row| {
            let status_str: String = row.get("status")?;
            Ok(Job { id: row.get("id")?, job_type: row.get("job_type")?, slug: row.get("slug")?, status: JobStatus::from_db_str(&status_str), message: row.get("message")? })
        })?;
        Ok(rows.next().transpose()?)
    }
}

/// Content hash used by `docbrain-ingest::chunk` as the dedup key — exposed
/// here so both crates agree on exactly one hashing scheme.
pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_library(slug: &str) -> SqliteDocbrainStore {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library(slug, slug, None, Some("https://example.com/docs")).unwrap();
        store
    }

    #[test]
    fn add_library_is_idempotent_by_slug() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let id1 = store.add_library("next", "Next.js", None, Some("https://nextjs.org/docs")).unwrap();
        let id2 = store.add_library("next", "Next.js", Some("vercel/next.js"), Some("https://nextjs.org/docs")).unwrap();
        assert_eq!(id1, id2, "re-registering the same slug must not create a duplicate library");
        assert_eq!(store.list_libraries().unwrap().len(), 1);
    }

    #[test]
    fn upsert_doc_node_dedups_on_content_hash_within_a_version() {
        let store = store_with_library("next");
        let hash = content_hash("Install the package first.");
        let node = NewDocNode { version: "15.1.2", topic: "Getting Started", content: "Install the package first.", content_hash: &hash, token_count: 5, embedding: None, kind: NodeKind::Prose };

        let first = store.upsert_doc_node("next", node.clone()).unwrap();
        assert!(matches!(first, UpsertOutcome::Inserted(_)));

        let second = store.upsert_doc_node("next", node).unwrap();
        assert!(matches!(second, UpsertOutcome::AlreadyExists(_)));
        assert_eq!(first.node_id(), second.node_id());
        assert_eq!(store.all_doc_nodes("next").unwrap().len(), 1, "re-scraping identical content must not duplicate the node");
    }

    #[test]
    fn different_content_at_same_topic_is_not_deduped() {
        let store = store_with_library("next");
        let hash_a = content_hash("content a");
        let hash_b = content_hash("content b");
        store.upsert_doc_node("next", NewDocNode { version: "1.0", topic: "routing", content: "content a", content_hash: &hash_a, token_count: 2, embedding: None, kind: NodeKind::Prose }).unwrap();
        store.upsert_doc_node("next", NewDocNode { version: "1.0", topic: "routing", content: "content b", content_hash: &hash_b, token_count: 2, embedding: None, kind: NodeKind::Prose }).unwrap();
        assert_eq!(store.all_doc_nodes("next").unwrap().len(), 2);
    }

    #[test]
    fn edges_are_queryable_from_either_side() {
        let store = store_with_library("next");
        let hash_a = content_hash("a");
        let hash_b = content_hash("b");
        let a = store.upsert_doc_node("next", NewDocNode { version: "1.0", topic: "a", content: "a", content_hash: &hash_a, token_count: 1, embedding: None, kind: NodeKind::Prose }).unwrap().node_id();
        let b = store.upsert_doc_node("next", NewDocNode { version: "1.0", topic: "b", content: "b", content_hash: &hash_b, token_count: 1, embedding: None, kind: NodeKind::Prose }).unwrap().node_id();
        store.add_doc_edge(a, b, EdgeRelation::Sequence).unwrap();

        assert_eq!(store.node_edges(a).unwrap().len(), 1);
        assert_eq!(store.node_edges(b).unwrap().len(), 1);
    }

    #[test]
    fn changelog_upsert_is_idempotent_on_natural_key() {
        let store = store_with_library("next");
        let id1 = store.add_changelog_entry("next", "1.0", "1.1", "first sync", true).unwrap();
        let id2 = store.add_changelog_entry("next", "1.0", "1.1", "re-synced text", true).unwrap();
        assert_eq!(id1, id2, "re-syncing the same version pair must not duplicate the row");
        assert_eq!(store.get_changelog("next", "1.0", "1.1").unwrap().len(), 1);
    }

    #[test]
    fn search_similar_finds_the_nearest_embedding_and_respects_library_scope() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        store.add_library("a", "A", None, None).unwrap();
        store.add_library("b", "B", None, None).unwrap();

        let mut close = vec![1.0_f32; EMBEDDING_DIM];
        close[0] = 1.1;
        let far = vec![-1.0_f32; EMBEDDING_DIM];
        let query = vec![1.0_f32; EMBEDDING_DIM];

        let hash_close = content_hash("close");
        let hash_far = content_hash("far");
        store
            .upsert_doc_node("a", NewDocNode { version: "1.0", topic: "close", content: "close", content_hash: &hash_close, token_count: 1, embedding: Some(&close), kind: NodeKind::Prose })
            .unwrap();
        store
            .upsert_doc_node("b", NewDocNode { version: "1.0", topic: "far", content: "far", content_hash: &hash_far, token_count: 1, embedding: Some(&far), kind: NodeKind::Prose })
            .unwrap();

        let hits = store.search_similar(&query, 5, None, None).unwrap();
        assert_eq!(hits[0].node.topic, "close");

        let scoped = store.search_similar(&query, 5, Some("b"), None).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].node.topic, "far");
    }

    #[test]
    fn search_similar_excludes_a_node_kind_when_asked() {
        let store = store_with_library("next");
        let vector = vec![1.0_f32; EMBEDDING_DIM];
        let hash_prose = content_hash("prose content");
        let hash_code = content_hash("code content");
        store
            .upsert_doc_node("next", NewDocNode { version: "1.0", topic: "prose", content: "prose content", content_hash: &hash_prose, token_count: 1, embedding: Some(&vector), kind: NodeKind::Prose })
            .unwrap();
        store
            .upsert_doc_node(
                "next",
                NewDocNode { version: "1.0", topic: "code", content: "code content", content_hash: &hash_code, token_count: 1, embedding: Some(&vector), kind: NodeKind::CodeExample },
            )
            .unwrap();

        let all = store.search_similar(&vector, 5, None, None).unwrap();
        assert_eq!(all.len(), 2);

        let prose_only = store.search_similar(&vector, 5, None, Some(NodeKind::CodeExample)).unwrap();
        assert_eq!(prose_only.len(), 1);
        assert_eq!(prose_only[0].node.kind, NodeKind::Prose);
    }

    #[test]
    fn job_lifecycle() {
        let store = SqliteDocbrainStore::open_in_memory().unwrap();
        let id = store.create_job("scrape_library", "next").unwrap();
        assert_eq!(store.get_job(id).unwrap().unwrap().status, JobStatus::Running);
        store.complete_job(id, JobStatus::Succeeded, "done").unwrap();
        assert_eq!(store.get_job(id).unwrap().unwrap().status, JobStatus::Succeeded);
    }
}
