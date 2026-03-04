# Library Database

The library subsystem stores all book metadata, reading progress, and
table-of-contents data in SQLite. This page explains the schema, the key
database types, and how data flows from disk into the database.

## Schema overview

The database is created and versioned by the SQL migration files in
`crates/core/migrations/`. The initial schema defines eleven tables plus one
aggregating view:

```mermaid
erDiagram
    books {
        TEXT fingerprint PK
        TEXT title
        TEXT file_path
        TEXT file_kind
        INTEGER file_size
        INTEGER added_at
    }

    authors {
        INTEGER id PK
        TEXT name
    }

    book_authors {
        TEXT book_fingerprint FK
        INTEGER author_id FK
        INTEGER position
    }

    categories {
        INTEGER id PK
        TEXT name
    }

    book_categories {
        TEXT book_fingerprint FK
        INTEGER category_id FK
    }

    reading_states {
        TEXT fingerprint PK
        INTEGER opened
        INTEGER current_page
        INTEGER pages_count
        INTEGER finished
    }

    thumbnails {
        TEXT fingerprint PK
        BLOB thumbnail_data
    }

    toc_entries {
        TEXT id PK
        TEXT book_fingerprint FK
        TEXT parent_id FK
        INTEGER position
        TEXT title
        TEXT location_kind
    }

    libraries {
        INTEGER id PK
        TEXT path
        TEXT name
        INTEGER created_at
    }

    library_books {
        INTEGER library_id FK
        TEXT book_fingerprint FK
        INTEGER added_to_library_at
    }

    _cadmus_migrations {
        TEXT id PK
        INTEGER executed_at
        TEXT status
    }

    books ||--o{ book_authors : ""
    authors ||--o{ book_authors : ""
    books ||--o{ book_categories : ""
    categories ||--o{ book_categories : ""
    books ||--o| reading_states : ""
    books ||--o| thumbnails : ""
    books ||--o{ toc_entries : ""
    toc_entries ||--o{ toc_entries : "parent_id"
    libraries ||--o{ library_books : ""
    books ||--o{ library_books : ""
```

### Key design choices

- **`books` is the main table.** Every other per-book table references
  `books.fingerprint` with `ON DELETE CASCADE`, so deleting a book row removes
  all associated data automatically.
- **Authors are normalised.** `authors` holds unique author names;
  `book_authors` is the join table and carries a `position` column that
  preserves display order.
- **All tables use `STRICT` mode.** SQLite's `STRICT` pragma enforces column
  type constraints at the storage layer, catching type mismatches early.
- **Timestamps are Unix epoch integers.** `added_at`, `created_at`, and similar
  columns are `INTEGER NOT NULL`; never `TEXT`.
- **TOC tree via adjacency list.** `toc_entries.parent_id` is a self-reference;
  `position` preserves sibling order. The `id` is a UUID7 (generated in Rust) so
  `ORDER BY id ASC` gives stable insertion order without a growing rowid.
- **`library_books_full_info` view.** An aggregating view joins `books`,
  `reading_states`, `book_authors`, `authors`, `book_categories`, and
  `categories` in one query. The `library_id` column from `library_books` is
  exposed so callers can filter with a plain `WHERE library_id = ?`.

## Data access layer

The
<a href="/api/cadmus_core/library/db/struct.Db">`cadmus_core::library::db::Db`</a>
struct is the entry point for all library database operations. It wraps the
shared `SqlitePool` and exposes a **synchronous** API by bridging every async
SQLx call through the global Tokio runtime:

```mermaid
flowchart LR
    caller["Caller (sync event loop)"]
    Db["library::db::Db"]
    RUNTIME["RUNTIME.block_on()"]
    SQLx["SQLx async query"]
    SQLite[("SQLite")]

    caller -->|sync call| Db
    Db -->|wraps in| RUNTIME
    RUNTIME -->|awaits| SQLx
    SQLx -->|reads/writes| SQLite
    SQLx -->|result| RUNTIME
    RUNTIME -->|returns| caller
```

The sync bridge exists because Cadmus's UI event loop is single-threaded and
synchronous. The global `RUNTIME` (a `tokio::runtime::Runtime` singleton) lets
the rest of the codebase call database methods without needing to be async.

Key methods on `Db`:

| Method                                                                                               | Purpose                                              |
| ---------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| <a href="/api/cadmus_core/library/db/struct.Db#method.register_library">`register_library`</a>       | Insert a new library row and return its id           |
| <a href="/api/cadmus_core/library/db/struct.Db#method.get_library_by_path">`get_library_by_path`</a> | Look up a library id by filesystem path              |
| <a href="/api/cadmus_core/library/db/struct.Db#method.get_all_books">`get_all_books`</a>             | Fetch every book in a library via the full-info view |
| <a href="/api/cadmus_core/library/db/struct.Db#method.insert_book">`insert_book`</a>                 | Write a new book and its authors/categories          |
| <a href="/api/cadmus_core/library/db/struct.Db#method.save_reading_state">`save_reading_state`</a>   | Save or update reading progress for a book           |
| <a href="/api/cadmus_core/library/db/struct.Db#method.save_toc">`save_toc`</a>                       | Bulk-write a book's table of contents                |
| <a href="/api/cadmus_core/library/db/struct.Db#method.get_thumbnail">`get_thumbnail`</a>             | Retrieve the stored cover thumbnail BLOB             |
| <a href="/api/cadmus_core/library/db/struct.Db#method.save_thumbnail">`save_thumbnail`</a>           | Save or replace a cover thumbnail                    |

## How a book scan flows into the database

When a library directory is scanned, Cadmus follows this sequence:

```mermaid
sequenceDiagram
    participant Scanner as Library Scanner
    participant Db as library::db::Db
    participant SQLite

    Scanner->>Db: register_library(path, name)
    Db->>SQLite: INSERT INTO libraries
    SQLite-->>Db: library_id

    loop for each book file
        Scanner->>Db: insert_book(library_id, fp, info)
        Db->>SQLite: INSERT INTO books
        Db->>SQLite: INSERT INTO authors / book_authors
        Db->>SQLite: INSERT INTO book_categories
        Db->>SQLite: INSERT INTO library_books
    end

    loop for each book with reading progress
        Scanner->>Db: save_reading_state(fp, reader_info)
        Db->>SQLite: INSERT OR REPLACE INTO reading_states
    end

    loop for each book with a TOC
        Scanner->>Db: save_toc(fp, entries)
        Db->>SQLite: INSERT INTO toc_entries
    end
```

## Related pages

- [SQLite & SQLx](sqlite-sqlx.md) — compile-time query verification, review rules
- [Runtime Migrations](runtime-migrations.md) — one-time data migrations using
  the `migration!` macro
