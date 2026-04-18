-- First, drop the view so we can modify the underlying tables.
DROP VIEW IF EXISTS library_books_full_info;

-- 1. Move absolute_path from books to library_books (per-library).
--
-- The same book (fingerprint) can exist in multiple libraries at
-- different paths, so absolute_path must be stored per-library row.

ALTER TABLE library_books ADD COLUMN absolute_path TEXT NOT NULL DEFAULT '';

-- Copy existing absolute paths from books before dropping the column.
UPDATE library_books SET absolute_path = (
    SELECT b.absolute_path FROM books b
    WHERE b.fingerprint = library_books.book_fingerprint
);

ALTER TABLE books DROP COLUMN absolute_path;

-- 2. Add pre-computed integer sort ranks for DB-level pagination.
--
-- Seven sort methods require Rust-side logic (natural sort, alphabetic
-- normalisation, series number parsing, status/progress bucketing) and so
-- cannot be derived from existing columns alone. These ranks are filled by
-- LibraryDb::compute_sort_keys() after every import.

ALTER TABLE library_books ADD COLUMN sort_title    INTEGER;
ALTER TABLE library_books ADD COLUMN sort_author   INTEGER;
ALTER TABLE library_books ADD COLUMN sort_filepath INTEGER;
ALTER TABLE library_books ADD COLUMN sort_filename INTEGER;
ALTER TABLE library_books ADD COLUMN sort_series   INTEGER;

-- Recreate the view with all necessary columns and indices.

CREATE VIEW IF NOT EXISTS library_books_full_info AS
SELECT
    lb.library_id,
    b.fingerprint,
    b.title,
    b.subtitle,
    b.year,
    b.language,
    b.publisher,
    b.series,
    b.edition,
    b.volume,
    b.number,
    b.identifier,
    lb.file_path,
    lb.absolute_path,
    b.file_kind,
    b.file_size,
    b.added_at,
    rs.opened,
    rs.current_page,
    rs.pages_count,
    rs.finished,
    rs.dithered,
    rs.zoom_mode,
    rs.scroll_mode,
    rs.page_offset_x,
    rs.page_offset_y,
    rs.rotation,
    rs.cropping_margins_json,
    rs.margin_width,
    rs.screen_margin_width,
    rs.font_family,
    rs.font_size,
    rs.text_align,
    rs.line_height,
    rs.contrast_exponent,
    rs.contrast_gray,
    rs.page_names_json,
    rs.bookmarks_json,
    rs.annotations_json,
    GROUP_CONCAT(DISTINCT a.name ORDER BY ba.position) AS authors,
    GROUP_CONCAT(DISTINCT c.name)                      AS categories,
    lb.sort_title,
    lb.sort_author,
    lb.sort_filepath,
    lb.sort_filename,
    lb.sort_series
FROM library_books lb
INNER JOIN books b           ON lb.book_fingerprint  = b.fingerprint
LEFT JOIN reading_states   rs ON b.fingerprint       = rs.fingerprint
LEFT JOIN book_authors     ba ON b.fingerprint       = ba.book_fingerprint
LEFT JOIN authors           a ON ba.author_id        = a.id
LEFT JOIN book_categories  bc ON b.fingerprint       = bc.book_fingerprint
LEFT JOIN categories        c ON bc.category_id      = c.id
GROUP BY lb.library_id, b.fingerprint;

-- Create indices for sort columns to support efficient pagination.
CREATE INDEX IF NOT EXISTS idx_library_books_sort_title    ON library_books(library_id, sort_title);
CREATE INDEX IF NOT EXISTS idx_library_books_sort_author   ON library_books(library_id, sort_author);
CREATE INDEX IF NOT EXISTS idx_library_books_sort_filepath ON library_books(library_id, sort_filepath);
CREATE INDEX IF NOT EXISTS idx_library_books_sort_filename ON library_books(library_id, sort_filename);
CREATE INDEX IF NOT EXISTS idx_library_books_sort_series   ON library_books(library_id, sort_series);
