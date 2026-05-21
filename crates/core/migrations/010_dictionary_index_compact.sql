-- Compact dictionary index schema.
--
-- Replaces the v1 schema (64-byte fingerprint per row, regular rowid table)
-- with a compact schema: small integer dict_id FK and WITHOUT ROWID.
--
-- The PRIMARY KEY includes offset so that multiple definitions sharing the
-- same normalized headword (e.g. "Pain", "PAIN", "pain" all normalizing to
-- "pain") are each stored as a separate row and all returned on lookup.

DROP TABLE IF EXISTS dictionary_index_entry;
DROP TABLE IF EXISTS dictionary_index_meta;

CREATE TABLE dictionary_index_meta (
    dict_id       INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint   TEXT    NOT NULL UNIQUE,
    dict_path     TEXT    NOT NULL,
    total_lines   INTEGER NOT NULL DEFAULT 0,
    indexed_lines INTEGER NOT NULL DEFAULT 0,
    completed     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE dictionary_index_entry (
    dict_id   INTEGER NOT NULL REFERENCES dictionary_index_meta(dict_id) ON DELETE CASCADE,
    word      TEXT    NOT NULL,
    offset    INTEGER NOT NULL,
    size      INTEGER NOT NULL,
    original  TEXT,
    PRIMARY KEY (dict_id, word, offset)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_dictionary_entry_word
    ON dictionary_index_entry(word);
