---
description: "Require sqlx query macros for compile-time SQL verification"
applyTo: "**/*.rs"
---

# SQLx Query Macros

All SQLx queries must use the typed macros (`query!`, `query_as!`,
`query_scalar!`). Never use the untyped `query()` / `query_as()` /
`query_scalar()` functions.

## Rationale

The macros verify SQL syntax and column types against the database schema at
compile time, catching mistakes before they reach runtime.

## Rules

- Use `sqlx::query!` for `INSERT`, `UPDATE`, `DELETE`, and `SELECT` that return
  raw rows
- Use `sqlx::query_as!` when mapping results directly into a named struct
- Use `sqlx::query_scalar!` for single-column results; call `.flatten()` on the
  result when the column is nullable (`Option<Option<T>>` → `Option<T>`)

## Exception: dynamic queries covered by tests

Untyped `sqlx::query()`, `sqlx::query_as()`, and `sqlx::query_scalar()` are
allowed **only** when the SQL cannot be expressed as a static string (e.g. a
dynamic `ORDER BY` column).  In that case:

1. The function **must** have unit tests that exercise every code path that
   touches the dynamic SQL.
2. Add a comment explaining why the typed macro cannot be used.

## Examples

✅ Good:

```rust
sqlx::query!(
    "INSERT OR IGNORE INTO authors (name) VALUES (?)",
    name
)
.execute(pool)
.await?;

let id: i64 = sqlx::query_scalar!("SELECT id FROM authors WHERE name = ?", name)
    .fetch_one(pool)
    .await?;

// Nullable column requires .flatten()
let existing: Option<i64> =
    sqlx::query_scalar!("SELECT id FROM libraries WHERE path = ?", path)
        .fetch_optional(pool)
        .await?
        .flatten();
```

✅ Also acceptable (dynamic ORDER BY, covered by tests):

```rust
// ORDER BY column is determined at runtime — typed macro requires a static
// SQL string, so untyped query_as is used here instead.
let rows: Vec<BookRow> = sqlx::query_as(&format!(
    "SELECT ... FROM books ORDER BY {col} {dir} LIMIT ? OFFSET ?"
))
.bind(limit)
.bind(offset)
.fetch_all(pool)
.await?;
```

❌ Bad:

```rust
sqlx::query("INSERT OR IGNORE INTO authors (name) VALUES (?)")
    .bind(name)
    .execute(pool)
    .await?;
```

