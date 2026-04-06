CREATE TABLE IF NOT EXISTS dictionary_monolingual_metadata (
    lang      TEXT    PRIMARY KEY NOT NULL,
    formats   TEXT    NOT NULL,
    updated   INTEGER NOT NULL,
    words     INTEGER NOT NULL,
    cached_at INTEGER NOT NULL
) STRICT;
