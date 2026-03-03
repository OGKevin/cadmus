CREATE TABLE IF NOT EXISTS books (
    fingerprint TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    subtitle TEXT NOT NULL DEFAULT '',
    year TEXT NOT NULL DEFAULT '',
    language TEXT NOT NULL DEFAULT '',
    publisher TEXT NOT NULL DEFAULT '',
    series TEXT NOT NULL DEFAULT '',
    edition TEXT NOT NULL DEFAULT '',
    volume TEXT NOT NULL DEFAULT '',
    number TEXT NOT NULL DEFAULT '',
    identifier TEXT NOT NULL DEFAULT '',
    file_path TEXT NOT NULL,
    file_kind TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    added_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS authors (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT UNIQUE NOT NULL
) STRICT;

-- Books can have multiple authors; position preserves display order.
CREATE TABLE IF NOT EXISTS book_authors (
    book_fingerprint TEXT NOT NULL,
    author_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (book_fingerprint, author_id),
    FOREIGN KEY (book_fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE,
    FOREIGN KEY (author_id) REFERENCES authors(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS idx_book_authors_book ON book_authors(book_fingerprint);
CREATE INDEX IF NOT EXISTS idx_book_authors_author ON book_authors(author_id);
CREATE INDEX IF NOT EXISTS idx_authors_name ON authors(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS categories (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name TEXT UNIQUE NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS book_categories (
    book_fingerprint TEXT NOT NULL,
    category_id INTEGER NOT NULL,
    PRIMARY KEY (book_fingerprint, category_id),
    FOREIGN KEY (book_fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE,
    FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS idx_book_categories_book ON book_categories(book_fingerprint);
CREATE INDEX IF NOT EXISTS idx_book_categories_category ON book_categories(category_id);
CREATE INDEX IF NOT EXISTS idx_categories_name ON categories(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS reading_states (
    fingerprint TEXT PRIMARY KEY NOT NULL,
    opened INTEGER NOT NULL,
    current_page INTEGER NOT NULL,
    pages_count INTEGER NOT NULL,
    finished INTEGER NOT NULL CHECK(finished IN (0, 1)),
    dithered INTEGER NOT NULL CHECK(dithered IN (0, 1)),
    zoom_mode TEXT,
    scroll_mode TEXT,
    page_offset_x INTEGER,
    page_offset_y INTEGER,
    rotation INTEGER,
    cropping_margins_json TEXT,
    margin_width INTEGER,
    screen_margin_width INTEGER,
    font_family TEXT,
    font_size REAL,
    text_align TEXT,
    line_height REAL,
    contrast_exponent REAL,
    contrast_gray REAL,
    page_names_json TEXT,
    bookmarks_json TEXT,
    annotations_json TEXT,
    FOREIGN KEY (fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE
) STRICT;

CREATE TABLE IF NOT EXISTS thumbnails (
    fingerprint TEXT PRIMARY KEY NOT NULL,
    thumbnail_data BLOB NOT NULL,
    FOREIGN KEY (fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE
) STRICT;

-- Each TOC entry is one row; the tree is encoded via parent_id + position.
-- location_kind discriminates between page-number and URI locations.
-- id is a UUID7 (TEXT) generated in Rust so ORDER BY id ASC preserves
-- insertion order without an ever-growing AUTOINCREMENT rowid.
CREATE TABLE IF NOT EXISTS toc_entries (
    id               TEXT    PRIMARY KEY NOT NULL,
    book_fingerprint TEXT    NOT NULL,
    parent_id        TEXT,
    position         INTEGER NOT NULL,
    title            TEXT    NOT NULL,
    location_kind    TEXT    NOT NULL CHECK (location_kind IN ('exact', 'uri')),
    location_exact   INTEGER,
    location_uri     TEXT,
    FOREIGN KEY (book_fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE,
    FOREIGN KEY (parent_id)        REFERENCES toc_entries(id)    ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS idx_toc_entries_book   ON toc_entries(book_fingerprint);

CREATE TABLE IF NOT EXISTS libraries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS library_books (
    library_id INTEGER NOT NULL,
    book_fingerprint TEXT NOT NULL,
    added_to_library_at INTEGER NOT NULL DEFAULT (unixepoch('now')),
    PRIMARY KEY (library_id, book_fingerprint),
    FOREIGN KEY (library_id) REFERENCES libraries(id) ON DELETE CASCADE,
    FOREIGN KEY (book_fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS idx_library_books_library ON library_books(library_id);
CREATE INDEX IF NOT EXISTS idx_library_books_book ON library_books(book_fingerprint);

CREATE TABLE IF NOT EXISTS _cadmus_migrations (
    id TEXT PRIMARY KEY NOT NULL,
    executed_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('success', 'failed'))
);

-- Aggregates all per-book data needed for get_all_books into a single query.
-- Including library_books in the FROM clause exposes library_id as a filterable
-- column so callers can do a plain WHERE library_id = ? with no explicit JOIN.
-- Authors are ordered by position; categories have no defined order.
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
    GROUP_CONCAT(DISTINCT a.name ORDER BY ba.position) AS authors,
    GROUP_CONCAT(DISTINCT c.name)                      AS categories
FROM library_books lb
INNER JOIN books b          ON lb.book_fingerprint  = b.fingerprint
LEFT JOIN reading_states   rs ON b.fingerprint       = rs.fingerprint
LEFT JOIN book_authors     ba ON b.fingerprint       = ba.book_fingerprint
LEFT JOIN authors           a ON ba.author_id        = a.id
LEFT JOIN book_categories  bc ON b.fingerprint       = bc.book_fingerprint
LEFT JOIN categories        c ON bc.category_id      = c.id
GROUP BY lb.library_id, b.fingerprint;
