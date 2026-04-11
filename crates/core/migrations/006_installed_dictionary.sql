CREATE TABLE IF NOT EXISTS dictionary_installed (
    lang              TEXT    PRIMARY KEY NOT NULL,
    installed_at      INTEGER NOT NULL,
    installed_version INTEGER NOT NULL
) STRICT;
