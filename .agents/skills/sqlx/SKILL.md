---
name: sqlx
description: Regenerate `.sqlx/` cached metadata when adding or modifying `sqlx::query!`, `sqlx::query_as!`, or `sqlx::query_scalar!` macros.
---

Project conventions and testing policy: [AGENTS.md](../../AGENTS.md). See the
SQLite / sqlx conventions section for query and newtype patterns.

# SQLx Offline Query Cache

Regenerate `.sqlx/` cached metadata when adding or modifying `sqlx::query!`,
`sqlx::query_as!`, or `sqlx::query_scalar!` macros.

## When to use

- New typed SQLx macro introduced
- Existing query string changed
- Schema migration added or altered

## Prerequisites

- `DATABASE_URL` must point to a migrated SQLite database
- `sqlx-cli` must be available

## Command

```bash
cargo sqlx prepare --all --workspace -- --tests
```

The `-- --tests` flag ensures `#[cfg(test)]` queries are also cached.

## Verification

After regenerating:

```bash
cargo check --all-targets
cargo test --workspace
```

Commit the updated `.sqlx/` files alongside the code changes.
