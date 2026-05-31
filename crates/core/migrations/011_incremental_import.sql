-- Add mtime (unix timestamp, ceiling-rounded to 2-second FAT32 precision)
-- and file_size to library_books for incremental import support.
-- Existing rows get NULL values, which are treated as "rescan needed".
ALTER TABLE library_books ADD COLUMN mtime INTEGER;
ALTER TABLE library_books ADD COLUMN file_size INTEGER;

-- Index on absolute_path to support efficient per-path mtime/size lookups
-- during incremental import scans.
CREATE INDEX IF NOT EXISTS idx_library_books_absolute_path
    ON library_books (library_id, absolute_path);
