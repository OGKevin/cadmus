-- Store the on-disk stamp of each indexed .index file so a dictionary reload
-- can skip hashing when the file has not changed.
--
-- Reusing the stored fingerprint is only correct while the file it was
-- computed from is still the file on disk. Reload previously hashed every
-- .index on every call, which stalled the UI on large dictionaries.
-- SQLite allows only one ADD COLUMN per ALTER TABLE.
ALTER TABLE dictionary_index_meta
    ADD COLUMN index_mtime INTEGER;

ALTER TABLE dictionary_index_meta
    ADD COLUMN index_size INTEGER;

-- Rows written before this migration have no stamp. NULL never matches a
-- current stamp, so those dictionaries rehash until the Rust backfill
-- migration `v1_dictionary_index_stamp` records their stamp from disk.
