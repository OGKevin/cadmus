-- Rebuildable dictionary word index: per-dictionary metadata and word entries.

CREATE TABLE IF NOT EXISTS dictionary_index_meta (
    dict_id       INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint   TEXT    NOT NULL UNIQUE,
    dict_path     TEXT    NOT NULL,
    total_lines   INTEGER NOT NULL DEFAULT 0,
    indexed_lines INTEGER NOT NULL DEFAULT 0,
    completed     INTEGER NOT NULL DEFAULT 0,
    index_mtime   INTEGER,
    index_size    INTEGER
);

CREATE TABLE IF NOT EXISTS dictionary_index_entry (
    dict_id   INTEGER NOT NULL REFERENCES dictionary_index_meta(dict_id) ON DELETE CASCADE,
    word      TEXT    NOT NULL,
    offset    INTEGER NOT NULL,
    size      INTEGER NOT NULL,
    original  TEXT,
    PRIMARY KEY (dict_id, word, offset)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_dictionary_entry_word
    ON dictionary_index_entry(word);
