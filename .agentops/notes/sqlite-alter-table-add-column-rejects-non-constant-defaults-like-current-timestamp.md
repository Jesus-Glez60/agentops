---
title: "SQLite ALTER TABLE ADD COLUMN rejects non-constant defaults like CURRENT_TIMESTAMP"
type: gotcha
---

Planned to add edges.updated_at via ALTER TABLE edges ADD COLUMN updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, matching how scan_history.started_at already works in CREATE TABLE. Confirmed via SQLite's own C source (alter.c) that ADD COLUMN explicitly validates the default through sqlite3ValueFromExpr() and rejects anything non-constant, erroring 'Cannot add a column with non-constant default' -- CURRENT_TIMESTAMP is evaluated per-row, not a fixed literal, so this would have failed the migration outright the first time it ran against a real populated database. Caught before implementing, not after, by checking SQLite's docs/source via context7 rather than assuming the CREATE TABLE default syntax carries over to ALTER TABLE. Fixed with a constant placeholder default (DEFAULT '') plus an immediate UPDATE ... SET updated_at = CURRENT_TIMESTAMP WHERE updated_at = '' backfill in the same migration step -- UPDATE has no such restriction. Every future add_edge INSERT sets updated_at explicitly going forward, so the '' placeholder is only ever seen by that one backfill.
