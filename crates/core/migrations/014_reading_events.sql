-- Table for individual reading events
CREATE TABLE IF NOT EXISTS reading_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    book_fingerprint TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    event_type TEXT NOT NULL CHECK(event_type IN ('BookOpened', 'BookClosed', 'PageTurn')),
    FOREIGN KEY (book_fingerprint) REFERENCES books(fingerprint) ON DELETE CASCADE
) STRICT;

CREATE INDEX IF NOT EXISTS idx_reading_events_book_fingerprint 
    ON reading_events(book_fingerprint);
CREATE INDEX IF NOT EXISTS idx_reading_events_timestamp 
    ON reading_events(timestamp);