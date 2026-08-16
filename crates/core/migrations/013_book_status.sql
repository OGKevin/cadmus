-- Add books.status for lifecycle (pending discovery vs active shelf books).
DROP VIEW IF EXISTS library_books_full_info;

ALTER TABLE books ADD COLUMN status TEXT NOT NULL
    DEFAULT 'pending_discovery'
    CHECK (status IN ('pending_discovery', 'active'));

-- Existing real books keep the shelf; stubs (empty file_kind) stay pending.
UPDATE books SET status = 'active' WHERE file_kind != '';

CREATE INDEX IF NOT EXISTS idx_books_status ON books (status);

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
    b.status,
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
