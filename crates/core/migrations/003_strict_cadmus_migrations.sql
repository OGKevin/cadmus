-- Drop and recreate _cadmus_migrations with STRICT mode.
-- SQLite does not support ALTER TABLE ... STRICT, so we drop and recreate.
-- Since only one migration tracking record exists at this point, losing that
-- data is acceptable.
DROP TABLE IF EXISTS _cadmus_migrations;

CREATE TABLE IF NOT EXISTS _cadmus_migrations (
    id TEXT PRIMARY KEY NOT NULL,
    executed_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('success', 'failed'))
) STRICT;
