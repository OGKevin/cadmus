-- Table for individual reading sessions
CREATE TABLE IF NOT EXISTS reading_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    book_fingerprint TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    FOREIGN KEY (book_fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE
) STRICT;

-- Index for querying sessions by book
CREATE INDEX IF NOT EXISTS idx_reading_sessions_book ON reading_sessions(book_fingerprint);

-- Index for time-based queries
CREATE INDEX IF NOT EXISTS idx_reading_sessions_started ON reading_sessions(started_at);

-- Table for per-book reading time aggregates
CREATE TABLE IF NOT EXISTS reading_time (
    book_fingerprint TEXT PRIMARY KEY NOT NULL,
    total_seconds INTEGER NOT NULL DEFAULT 0,
    last_session_start INTEGER,
    sessions_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (book_fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE
) STRICT;