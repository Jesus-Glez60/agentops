//! Shared SQLite adapter scaffolding: connection open/migrate helpers and a
//! generic natural-key upsert. Extracted so `docbrain-graph` and (once its
//! own foundation is rebuilt) `agentops-graph` share one implementation of
//! this boilerplate instead of independently re-copying and re-diverging on
//! it a third time — `agentops-graph` had a shared `upsert_node()` helper on
//! `main`, but `docbrain-graph` didn't and relied on a `UNIQUE` constraint
//! plus a manual check-then-act race in its caller instead. See
//! hexagonal-architecture-guide.md's noted layering gap for the history.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Opens (creating parent directories if needed) a file-backed connection
/// and runs `migrations` (a `CREATE TABLE IF NOT EXISTS ...` batch) against
/// it.
pub fn open(path: &Path, migrations: &str) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating parent dir for {}", path.display()))?;
    }
    let conn = Connection::open(path).with_context(|| format!("opening sqlite db at {}", path.display()))?;
    conn.execute_batch(migrations).context("running migrations")?;
    Ok(conn)
}

/// Same as [`open`], but in-memory — for tests and ephemeral use.
pub fn open_in_memory(migrations: &str) -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(migrations).context("running migrations")?;
    Ok(conn)
}

/// Upserts one row into `table`, keyed on `key_cols`, using SQLite's
/// `INSERT ... ON CONFLICT (...) DO UPDATE` — a real atomic upsert, not the
/// check-then-act race a plain `INSERT` guarded by a `UNIQUE` constraint
/// forces callers into. Returns the row's `rowid` either way (inserted or
/// already-existing-and-updated).
///
/// `cols`/`values` must be the same length and order; `key_cols` must be a
/// subset of `cols`.
pub fn upsert(conn: &Connection, table: &str, cols: &[&str], key_cols: &[&str], values: &[&dyn rusqlite::ToSql]) -> Result<i64> {
    assert_eq!(cols.len(), values.len(), "cols and values must line up");

    let col_list = cols.join(", ");
    let placeholders = (1..=cols.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let conflict = key_cols.join(", ");
    let update_cols: Vec<String> = cols.iter().filter(|c| !key_cols.contains(c)).map(|c| format!("{c} = excluded.{c}")).collect();

    let sql = if update_cols.is_empty() {
        format!("INSERT INTO {table} ({col_list}) VALUES ({placeholders}) ON CONFLICT ({conflict}) DO NOTHING")
    } else {
        format!("INSERT INTO {table} ({col_list}) VALUES ({placeholders}) ON CONFLICT ({conflict}) DO UPDATE SET {}", update_cols.join(", "))
    };
    conn.execute(&sql, values).with_context(|| format!("upserting into {table}"))?;

    // `last_insert_rowid()` is unreliable after a `DO UPDATE`/`DO NOTHING`
    // no-op (it isn't updated on a no-op conflict path) — look the row back
    // up by its natural key instead, which is correct in every case.
    let where_clause = key_cols.iter().enumerate().map(|(i, c)| format!("{c} = ?{}", i + 1)).collect::<Vec<_>>().join(" AND ");
    let key_values: Vec<&dyn rusqlite::ToSql> = key_cols
        .iter()
        .map(|k| {
            let idx = cols.iter().position(|c| c == k).expect("key_col must be in cols");
            values[idx]
        })
        .collect();
    let id: i64 = conn
        .query_row(&format!("SELECT rowid FROM {table} WHERE {where_clause}"), key_values.as_slice(), |r| r.get(0))
        .with_context(|| format!("reading back upserted row from {table}"))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = "
        CREATE TABLE widgets (
            id    INTEGER PRIMARY KEY AUTOINCREMENT,
            slug  TEXT NOT NULL UNIQUE,
            name  TEXT NOT NULL
        );
    ";

    #[test]
    fn upsert_inserts_a_new_row() {
        let conn = open_in_memory(SCHEMA).unwrap();
        let id = upsert(&conn, "widgets", &["slug", "name"], &["slug"], &[&"react", &"React"]).unwrap();
        let name: String = conn.query_row("SELECT name FROM widgets WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(name, "React");
    }

    #[test]
    fn upsert_updates_an_existing_row_by_natural_key_without_duplicating() {
        let conn = open_in_memory(SCHEMA).unwrap();
        let id1 = upsert(&conn, "widgets", &["slug", "name"], &["slug"], &[&"react", &"React (old name)"]).unwrap();
        let id2 = upsert(&conn, "widgets", &["slug", "name"], &["slug"], &[&"react", &"React"]).unwrap();

        assert_eq!(id1, id2, "re-upserting the same natural key must return the same row, not a duplicate");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM widgets", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
        let name: String = conn.query_row("SELECT name FROM widgets WHERE id = ?1", [id1], |r| r.get(0)).unwrap();
        assert_eq!(name, "React");
    }

    #[test]
    fn open_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("db.sqlite3");
        let conn = open(&path, SCHEMA).unwrap();
        upsert(&conn, "widgets", &["slug", "name"], &["slug"], &[&"vue", &"Vue"]).unwrap();
        assert!(path.exists());
    }
}
