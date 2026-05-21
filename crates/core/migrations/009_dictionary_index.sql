-- Tracks per-dictionary indexing progress and completion state.
-- fingerprint is a 64-char BLAKE3 hex string (from the Fp type in helpers.rs).
CREATE TABLE IF NOT EXISTS dictionary_index_meta (
    -- BLAKE3 hex fingerprint of the .index file; identifies the dictionary.
    fingerprint    TEXT    NOT NULL PRIMARY KEY,
    -- Absolute path to the .index file on disk.
    dict_path      TEXT    NOT NULL,
    -- Total number of lines in the dictionary index file.
    total_lines    INTEGER NOT NULL DEFAULT 0,
    -- Number of lines processed so far; equals total_lines when complete.
    indexed_lines  INTEGER NOT NULL DEFAULT 0,
    -- 1 when indexing finished successfully, 0 while in progress or on failure.
    completed      INTEGER NOT NULL DEFAULT 0
);

-- The indexed word entries for fast dictionary lookups.
CREATE TABLE IF NOT EXISTS dictionary_index_entry (
    -- BLAKE3 hex fingerprint of the dictionary this entry belongs to.
    fingerprint  TEXT    NOT NULL REFERENCES dictionary_index_meta(fingerprint) ON DELETE CASCADE,
    -- Normalized headword used for lookup (lowercased / char-filtered per dictionary metadata).
    word         TEXT    NOT NULL,
    -- Byte offset of this entry's definition in the dictionary file.
    offset       INTEGER NOT NULL,
    -- Byte length of this entry's definition in the dictionary file.
    size         INTEGER NOT NULL,
    -- Pre-normalization headword; NULL when normalization did not change the word.
    -- Preserved so the UI can display the original casing/form after a match.
    original     TEXT,
    PRIMARY KEY (fingerprint, word)
);

-- Enables efficient word lookup across all dictionaries simultaneously.
CREATE INDEX IF NOT EXISTS idx_dictionary_entry_word
    ON dictionary_index_entry(word);
