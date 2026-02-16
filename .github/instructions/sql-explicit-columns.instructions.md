---
description: "Require explicit column lists in SQL queries"
applyTo: "**/*.rs"
---

# SQL Column Selection

All SQL queries must list explicit column names. Do not use `SELECT *`.

## Rationale

- Avoids accidental schema coupling
- Keeps SQLx `query_as!` checks aligned with returned columns
- Makes query intent clear during code review

## Examples

✅ Good:

```sql
SELECT id, name, created_at
FROM users
WHERE id = ?
```

❌ Bad:

```sql
SELECT *
FROM users
WHERE id = ?
```
