-- Add missing reading_states columns to library_books_full_info view.
-- This enables loading full reader info (dithering, zoom_mode, etc.) when fetching books.

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
    b.file_path,
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
    GROUP_CONCAT(DISTINCT c.name)                      AS categories
FROM library_books lb
INNER JOIN books b          ON lb.book_fingerprint   = b.fingerprint
LEFT JOIN reading_states   rs ON b.fingerprint       = rs.fingerprint
LEFT JOIN book_authors     ba ON b.fingerprint       = ba.book_fingerprint
LEFT JOIN authors           a ON ba.author_id        = a.id
LEFT JOIN book_categories  bc ON b.fingerprint       = bc.book_fingerprint
LEFT JOIN categories        c ON bc.category_id      = c.id
GROUP BY lb.library_id, b.fingerprint;