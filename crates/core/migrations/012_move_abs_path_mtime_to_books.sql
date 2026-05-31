-- Move absolute_path and mtime from library_books to books.
-- These are properties of the book file itself, not of the book-library
-- relationship, so they belong on the books table.

-- 1. Add columns to books (absolute_path may already exist if older SQLite
-- didn't support DROP COLUMN in migration 007).
ALTER TABLE books ADD COLUMN absolute_path TEXT NOT NULL DEFAULT '';
ALTER TABLE books ADD COLUMN mtime INTEGER;

-- 2. Populate from library_books. For books in multiple libraries, pick any row.
UPDATE books SET absolute_path = (
    SELECT lb.absolute_path FROM library_books lb
    WHERE lb.book_fingerprint = books.fingerprint
      AND lb.absolute_path != ''
    LIMIT 1
) WHERE absolute_path = '';

UPDATE books SET mtime = (
    SELECT lb.mtime FROM library_books lb
    WHERE lb.book_fingerprint = books.fingerprint
      AND lb.mtime IS NOT NULL
    LIMIT 1
) WHERE mtime IS NULL;

-- 3. Recreate the view to reflect the new column locations.
DROP VIEW IF EXISTS library_books_full_info;

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
    b.absolute_path,
    b.file_kind,
    b.file_size,
    b.added_at,
    b.mtime,
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
