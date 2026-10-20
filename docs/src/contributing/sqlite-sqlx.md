<!-- i18n:skip-start -->

# SQLite & SQLx

Cadmus uses [SQLite](https://sqlite.org) as its embedded database and
[SQLx](https://github.com/launchbadge/sqlx) as the Rust database library.
SQLx provides **compile-time SQL verification** — every query is checked against
the real schema before the code ships.

## Runtime databases

Cadmus opens two SQLite files beside each other:

- `cadmus.sqlite` — library data, installed-dictionary metadata, integrity
  checks, backups, and the library migrator.
- `dictionary.sqlite` — the rebuildable word index only. Library integrity
  checks and backups do not read this file. If connect or migrate fails, the
  file and its `-wal`/`-shm` companions are deleted and opened again so the
  index task can rebuild it.

Installed-dictionary rows stay in the library database. The word index lives
in `crates/core/dictionary/migrations/`.

## The `.sqlx` directory

The `.sqlx/` directory at the repository root contains one JSON metadata file
per SQL query. Each file stores the resolved column names, types, and parameter
types that SQLx inferred from the live database schema at the time
`cargo sqlx prepare` was last run.

```text
.sqlx/
├── query-10c2db2a….json   ← compile-time metadata for one query
├── query-13c26d81….json
└── …
```

### Regenerating query metadata

After adding or changing any SQL query, regenerate the metadata:

```bash
cargo xtask sqlx-prepare
```

The command creates a temporary SQLite file, applies `crates/core/migrations`
with `sqlx migrate run`, then applies `crates/core/dictionary/migrations` with
`sqlite3`. It runs `cargo sqlx prepare` against that file and rewrites
`.sqlx/`. Commit the updated files alongside your code change.

`cargo sqlx prepare` builds the crate once. Every `query!` macro in that build
reads one database URL
([sqlx#916](https://github.com/transact-rs/sqlx/issues/916)). Library queries
and dictionary index queries both live in `cadmus-core`, so prepare cannot use
the two runtime files as separate targets. The temporary file holds both
schemas. The dictionary migrator does not run on it: both migrators record
version 1 in `_sqlx_migrations`. The app still opens `dictionary.sqlite` at
startup and migrates that file on its own.

`DATABASE_URL` is not part of the devenv environment. Normal builds read
`.sqlx/` because `SQLX_OFFLINE=true`.

> [!IMPORTANT]
> If you forget to run `cargo xtask sqlx-prepare`, the CI `check` job will fail
> because the cached metadata will be out of date with your query changes.

## Compile-time SQL checking

SQLx's typed query macros (`query!`, `query_as!`, `query_scalar!`) verify SQL at
compile time using the metadata in `.sqlx/`. This means:

- Typos in column names are **compiler errors**, not runtime panics.
- Binding the wrong type to a `?` placeholder is a **type error**.
- Adding or removing a column in a migration without updating queries is caught
  **before deployment**.

Regular `cargo build` and `cargo check` do not open a database. They use the
pre-generated `.sqlx/` files. `cargo xtask sqlx-prepare` is the command that
talks to a live database.

> [!IMPORTANT]
> `.sqlx/` is only used when `SQLX_OFFLINE=true`, which devenv sets from
> `.env`.

## Review rules

The following rules are enforced during code review for all SQLx queries.

### Use typed macros only

Always use the typed macros. Never call the untyped `query()`, `query_as()`, or
`query_scalar()` functions:

| Goal                                       | Use                   |
| ------------------------------------------ | --------------------- |
| `INSERT`, `UPDATE`, `DELETE`, raw `SELECT` | `sqlx::query!`        |
| `SELECT` mapped into a named struct        | `sqlx::query_as!`     |
| Single-column `SELECT`                     | `sqlx::query_scalar!` |

When the column is nullable, call `.flatten()` on the result to collapse
`Option<Option<T>>` into `Option<T>`:

```rust
let id: Option<i64> =
    sqlx::query_scalar!("SELECT id FROM libraries WHERE path = ?", path)
        .fetch_optional(pool)
        .await?
        .flatten();
```

### List explicit column names

Never use `SELECT *`. Always name every column you need:

```sql
-- ✅ Good
SELECT id, path, name FROM libraries WHERE id = ?

-- ❌ Bad
SELECT * FROM libraries WHERE id = ?
```

### Store timestamps as Unix epoch integers

All date/time values must be stored as `INTEGER NOT NULL` (Unix epoch seconds).
Do not use `TEXT` columns for timestamps:

```sql
-- ✅ Good
created_at INTEGER NOT NULL

-- ❌ Bad
created_at TEXT NOT NULL DEFAULT (datetime('now'))
```

### Add only indexes that are actively used

Every index must be used by at least one query in the codebase. Unused indexes
waste write performance and storage without any read benefit. Before adding an
index, verify a query filters, sorts, or joins on the indexed column(s).

### Keep schema migration files stable

`cadmus-core` hashes every file under `crates/core/migrations/` at build time
and stores that hash in the database version stamp. Downgrade protection uses
the hash to allow newer-to-older app version moves only when both builds have
the same schema migration set.

Do not edit a schema migration after it has shipped. Add a new numbered
migration instead so existing databases keep passing SQLx migration checksum
validation.

## API reference

The primary database types live in the `cadmus_core::db` module:

- <a href="/api/cadmus_core/db/struct.Database">`cadmus_core::db::Database`</a> —
  the top-level sync/async bridge that owns the connection pool
- <a href="/api/cadmus_core/db/migrations/struct.MigrationRunner">`cadmus_core::db::migrations::MigrationRunner`</a> —
  executes all pending runtime migrations
- <a href="/api/cadmus_core/macro.migration">`cadmus_core::migration!`</a> —
  macro for declaring one-time runtime migrations

See [Library Database](library-database.md) for how the library subsystem uses
the database, and [Runtime Migrations](runtime-migrations.md) for how to write
one-time data migrations.

<!-- i18n:skip-end -->
