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

❌ Bad:

```rust
sqlx::query("INSERT OR IGNORE INTO authors (name) VALUES (?)")
    .bind(name)
    .execute(pool)
    .await?;
```
