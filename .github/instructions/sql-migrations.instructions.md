---
description: "Schema migration conventions for SQLite"
applyTo: "crates/core/migrations/**/*.sql"
---

# SQL Migration Conventions

## Timestamp Storage

All date/time values must be stored as **Unix epoch seconds** (`INTEGER NOT NULL`).
Never use `TEXT` for timestamps.

### Rationale

- SQLite has no native timestamp type; `INTEGER` is the idiomatic representation
- Enables correct numeric comparisons and ordering without string parsing
- Eliminates format ambiguity (`%Y-%m-%d %H:%M:%S` vs RFC 3339 vs ISO 8601)
- Maps directly to `chrono::NaiveDateTime::and_utc().timestamp()` / `DateTime::from_timestamp`
  with no intermediate string allocation

### Required pattern

```sql
-- ✅ Good
created_at INTEGER NOT NULL
added_at   INTEGER NOT NULL DEFAULT (unixepoch('now'))
```

```sql
-- ❌ Bad
created_at TEXT NOT NULL
added_at   TEXT NOT NULL DEFAULT (datetime('now'))
```

## Index Policy

Every index added to a migration **must be actively used** by at least one query
in the codebase. Unused indices waste write performance and storage without
providing any read benefit.

Before adding an index, verify that a query filters, sorts, or joins on the
indexed column(s). If no such query exists, do not add the index.

Before removing a query that uses an index, check whether the index is still
needed by another query. If not, remove the index in the same change.

```sql
-- ✅ Good: index supports a WHERE clause used in the codebase
CREATE INDEX IF NOT EXISTS idx_library_books_library ON library_books(library_id);

-- ❌ Bad: index on a column never referenced in any query
CREATE INDEX IF NOT EXISTS idx_books_title ON books(title COLLATE NOCASE);
```
