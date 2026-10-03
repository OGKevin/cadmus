-- Why this table exists: books keeps `fingerprint` (a 64-character hex string) as its
-- primary key, and a foreign key cannot target the implicit rowid because it has no
-- declared UNIQUE constraint. reading_events is append-heavy and would otherwise repeat
-- that 64-byte key in every row and index. book_keys provides a narrow, stable integer
-- key for tables that need one, without rebuilding books.
CREATE TABLE book_keys (
    book_id     INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint TEXT NOT NULL UNIQUE
                REFERENCES books(fingerprint) ON DELETE CASCADE
) STRICT;

CREATE TRIGGER book_keys_after_insert
AFTER INSERT ON books
BEGIN
    INSERT OR IGNORE INTO book_keys (fingerprint) VALUES (NEW.fingerprint);
END;

INSERT INTO book_keys (fingerprint) SELECT fingerprint FROM books;
