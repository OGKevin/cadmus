CREATE TABLE IF NOT EXISTS _cadmus_version (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    version        TEXT    NOT NULL,
    migration_hash TEXT    NOT NULL,
    migrated_at    INTEGER NOT NULL
) STRICT;
