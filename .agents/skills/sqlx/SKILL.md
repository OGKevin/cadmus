---
name: sqlx
description: Regenerate `.sqlx/` cached metadata when adding or modifying `sqlx::query!`, `sqlx::query_as!`, or `sqlx::query_scalar!` macros.
---

# SQLx Offline Query Cache

Regenerate `.sqlx/` cached metadata when adding or modifying `sqlx::query!`,
`sqlx::query_as!`, or `sqlx::query_scalar!` macros.

## When to use

- New typed SQLx macro introduced
- Existing query string changed
- Schema migration added or altered

## Prerequisites

- `sqlx-cli` and `sqlite3` on `PATH` (both are in the devenv shell)

Do not set `DATABASE_URL`. `cargo xtask sqlx-prepare` creates a temporary
SQLite file, applies `crates/core/migrations`, then applies
`crates/core/dictionary/migrations`, and points `cargo sqlx prepare` at that
file. One crate build can describe queries against only one URL
([sqlx#916](https://github.com/transact-rs/sqlx/issues/916)). The dictionary
migrator is not run on the temporary file: both migrators use version 1 in
`_sqlx_migrations`. The app migrates `dictionary.sqlite` on its own.

## Command

```bash
cargo xtask sqlx-prepare
```

The command passes `-- --tests` so `#[cfg(test)]` queries are cached, and sets
`SQLX_OFFLINE=false` for that process only. devenv keeps `SQLX_OFFLINE=true`
for normal builds.

## Verification

After regenerating:

```bash
cargo check --all-targets
cargo test --workspace
```

Commit the updated `.sqlx/` files alongside the code changes.
