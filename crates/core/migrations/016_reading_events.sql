DROP TABLE IF EXISTS reading_events;

CREATE TABLE reading_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    book_id    INTEGER NOT NULL REFERENCES book_keys(book_id) ON DELETE CASCADE,
    timestamp  INTEGER NOT NULL,
    event_type TEXT NOT NULL CHECK(event_type IN ('BookOpened', 'BookClosed', 'PageTurn'))
) STRICT;

CREATE INDEX idx_reading_events_book_id ON reading_events(book_id);
CREATE INDEX idx_reading_events_timestamp ON reading_events(timestamp);
