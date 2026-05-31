//! Runtime migrations for the library subsystem.
//!
//! Each migration is registered automatically at startup via the [`crate::migration!`]
//! macro and tracked in the `_cadmus_migrations` table.
//!
//! # Registered migrations
//!
//! | Module | Migration ID |
//! |---|---|
//! | [`import_legacy_filesystem_data::MIGRATION_ID`] | `v1_import_legacy_filesystem_data` |
//! | [`rehash_fingerprints::MIGRATION_ID`] | `v2_rehash_fingerprints` |

use crate::db::types::OptionalUuid7;
use crate::db::types::UnixTimestamp;
use crate::db::types::Uuid7;
use crate::device::CURRENT_DEVICE;
use crate::document::SimpleTocEntry;
use crate::helpers::{Fingerprint, Fp};
#[cfg(not(feature = "test"))]
use crate::library::THUMBNAIL_PREVIEWS_DIRNAME;
use crate::library::db::conversion::{
    encode_location, extract_authors, info_to_book_row, reader_info_to_reading_state_row,
    rows_to_toc_entries,
};
use crate::library::db::models::TocEntryRow;
use crate::library::{METADATA_FILENAME, READING_STATES_DIRNAME};
use crate::metadata::Info;
use crate::settings::versioned::SettingsManager;
use crate::version::get_current_version;
use fxhash::FxBuildHasher;
use indexmap::IndexMap;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tokio::fs;
use tracing::{error, info, warn};

crate::migration!(
    /// Imports book metadata from `.metadata.json` and reading progress from
    /// `.reading-states/<fingerprint>.json` into SQLite for every library path
    /// listed in `Settings.toml`. Covers both legacy library modes:
    ///
    /// - Database mode: had `.metadata.json` keyed by fingerprint.
    /// - Filesystem mode: no `.metadata.json`; only `.reading-states/` files.
    ///   Stub book rows are inserted to satisfy the foreign key before reading
    ///   states are written. A follow-up migration prunes stubs for missing files.
    ///
    /// The migration is idempotent (all inserts use `ON CONFLICT … DO NOTHING`).
    "v1_import_legacy_filesystem_data",
    async fn import_legacy_filesystem_data(pool: &SqlitePool) {
        let settings = SettingsManager::new(CURRENT_DEVICE.data_dir(), get_current_version()).load();

        if settings.libraries.is_empty() {
            info!("no libraries in settings, skipping legacy data import");
            return Ok(());
        }

        for lib in &settings.libraries {
            let library_path = &lib.path;
            let library_name = &lib.name;
            let path_str = library_path.to_string_lossy();

            info!(path = %path_str, name = %library_name, "importing legacy data for library");

            let library_id = ensure_library(pool, &path_str, library_name).await;

            let library_id = match library_id {
                Ok(id) => id,
                Err(e) => {
                    error!(path = %path_str, error = %e, "failed to register library, skipping");
                    continue;
                }
            };

            let (book_count, state_count) = import_library(pool, library_id, library_path).await;

            info!(
                path = %path_str,
                books_imported = book_count,
                reading_states_imported = state_count,
                "library import complete"
            );
        }

        Ok(())
    }
);

crate::migration!(
    /// Re-fingerprints every book in all libraries using BLAKE3 content hashing.
    ///
    /// The old fingerprint was derived from file metadata (mtime + size relative to
    /// the FAT32 epoch), which was unstable across timestamp changes. This migration
    /// computes a new content-based fingerprint for each file that is still present
    /// on disk and re-keys all associated database rows (reading states, thumbnails,
    /// TOC entries, authors, categories) to the new fingerprint, preserving user
    /// progress data.
    ///
    /// Files that are no longer present on disk keep a canonicalized legacy
    /// fingerprint in the database so their data remains readable until the next
    /// `import()` scan removes them as orphans.
    "v2_rehash_fingerprints",
    async fn rehash_fingerprints(pool: &SqlitePool) {
        let books: Vec<(String, Option<String>)> = sqlx::query(
                r#"
                SELECT
                    b.fingerprint,
                    (
                        SELECT lb.absolute_path
                        FROM library_books lb
                        WHERE lb.book_fingerprint = b.fingerprint
                          AND lb.absolute_path != ''
                        ORDER BY lb.absolute_path ASC, lb.library_id ASC
                        LIMIT 1
                    ) AS "absolute_path?: String"
                FROM books b
                "#
            )
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("fingerprint"),
                    row.get::<Option<String>, _>("absolute_path?: String"),
                )
            })
            .collect();

        for (old_fp_str, absolute_path) in &books {
            let Some(absolute_path) = absolute_path.as_ref() else {
                continue;
            };

            let abs_path = PathBuf::from(absolute_path);

            if !abs_path.exists() {
                continue;
            }

            let new_fp = match abs_path.fingerprint() {
                Ok(fp) => fp,
                Err(e) => {
                    error!(path = ?abs_path, error = %e, "failed to compute BLAKE3 fingerprint, skipping");
                    continue;
                }
            };

            let new_fp_str = new_fp.to_string();

            if new_fp_str == *old_fp_str {
                continue;
            }

            if let Err(e) = rekey_book(pool, old_fp_str, &new_fp_str).await {
                error!(
                    old_fp = %old_fp_str,
                    new_fp = %new_fp_str,
                    error = %e,
                    "failed to re-key book, skipping"
                );
            }
        }

        canonicalize_legacy_fingerprints(pool, &books).await?;

        Ok(())
    }
);

#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool, books)))]
async fn canonicalize_legacy_fingerprints(
    pool: &sqlx::SqlitePool,
    books: &[(String, Option<String>)],
) -> Result<(), anyhow::Error> {
    for (old_fp_str, _) in books {
        if old_fp_str.len() == 64 {
            continue;
        }

        let canonical_fp = match Fp::from_legacy_str(old_fp_str) {
            Ok(fp) => fp.to_string(),
            Err(_) => {
                warn!(
                    fingerprint = %old_fp_str,
                    "deleting malformed legacy fingerprint that cannot be canonicalized"
                );

                sqlx::query!("DELETE FROM books WHERE fingerprint = ?", old_fp_str)
                    .execute(pool)
                    .await?;

                continue;
            }
        };

        if let Err(e) = rekey_book(pool, old_fp_str, &canonical_fp).await {
            error!(
                old_fp = %old_fp_str,
                new_fp = %canonical_fp,
                error = %e,
                "failed to canonicalize legacy fingerprint"
            );
        }
    }

    Ok(())
}

/// Re-keys a single book row from `old_fp` to `new_fp`, preserving all
/// associated data (reading state, thumbnails, TOC, authors, categories).
///
/// All child tables use `ON DELETE CASCADE`, so deleting the old `books` row
/// at the end cascades cleanly. We update each child table explicitly first
/// to transfer data to the new fingerprint before the cascade fires.
///
/// The `library_books` UPDATE is intentionally global (not scoped to a single
/// library) because the subsequent DELETE cascades globally — scoping the
/// UPDATE would silently drop other libraries' associations.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn rekey_book(
    pool: &sqlx::SqlitePool,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    let mut tx = pool.begin().await?;

    let already_exists: Option<String> = sqlx::query_scalar!(
        "SELECT fingerprint FROM books WHERE fingerprint = ?",
        new_fp
    )
    .fetch_optional(&mut *tx)
    .await?;

    if already_exists.is_some() {
        merge_duplicate_book_data(&mut tx, old_fp, new_fp).await?;

        sqlx::query!("DELETE FROM books WHERE fingerprint = ?", old_fp)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        return Ok(());
    }

    insert_rekeyed_book(&mut tx, old_fp, new_fp).await?;
    move_rekeyed_book_data(&mut tx, old_fp, new_fp).await?;

    sqlx::query!("DELETE FROM books WHERE fingerprint = ?", old_fp)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn merge_duplicate_book_data(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    merge_library_books(tx, old_fp, new_fp).await?;
    merge_reading_states(tx, old_fp, new_fp).await?;
    merge_thumbnails(tx, old_fp, new_fp).await?;
    merge_toc_entries(tx, old_fp, new_fp).await?;
    merge_book_authors(tx, old_fp, new_fp).await?;
    merge_book_categories(tx, old_fp, new_fp).await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn merge_library_books(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO library_books (library_id, book_fingerprint, added_to_library_at, file_path, absolute_path)
         SELECT library_id, ?, added_to_library_at, file_path, absolute_path
         FROM library_books
         WHERE book_fingerprint = ?",
    )
    .bind(new_fp)
    .bind(old_fp)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn merge_reading_states(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO reading_states (
            fingerprint, opened, current_page, pages_count, finished, dithered,
            zoom_mode, scroll_mode, page_offset_x, page_offset_y, rotation,
            cropping_margins_json, margin_width, screen_margin_width,
            font_family, font_size, text_align, line_height,
            contrast_exponent, contrast_gray,
            page_names_json, bookmarks_json, annotations_json
        )
        SELECT
            ?, opened, current_page, pages_count, finished, dithered,
            zoom_mode, scroll_mode, page_offset_x, page_offset_y, rotation,
            cropping_margins_json, margin_width, screen_margin_width,
            font_family, font_size, text_align, line_height,
            contrast_exponent, contrast_gray,
            page_names_json, bookmarks_json, annotations_json
        FROM reading_states
        WHERE fingerprint = ?
        "#,
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn merge_thumbnails(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    sqlx::query!(
        "INSERT OR IGNORE INTO thumbnails (fingerprint, thumbnail_data)
         SELECT ?, thumbnail_data
         FROM thumbnails
         WHERE fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn merge_toc_entries(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    let existing_count: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM toc_entries WHERE book_fingerprint = ?",
        new_fp,
    )
    .fetch_one(&mut **tx)
    .await?;

    if existing_count > 0 {
        return Ok(());
    }

    let old_rows = sqlx::query_as!(
        TocEntryRow,
        r#"
        SELECT
            book_fingerprint,
            id                as "id: Uuid7",
            parent_id         as "parent_id!: OptionalUuid7",
            position,
            title,
            location_kind,
            location_exact,
            location_uri
        FROM toc_entries
        WHERE book_fingerprint = ?
        ORDER BY id ASC
        "#,
        old_fp,
    )
    .fetch_all(&mut **tx)
    .await?;

    if old_rows.is_empty() {
        return Ok(());
    }

    let toc_entries = rows_to_toc_entries(&old_rows)?;
    insert_toc_entries(tx, new_fp, &toc_entries, None).await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx, entries), fields(book_fingerprint = %book_fingerprint)))]
async fn insert_toc_entries(
    tx: &mut Transaction<'_, Sqlite>,
    book_fingerprint: &str,
    entries: &[SimpleTocEntry],
    parent_id: Option<Uuid7>,
) -> Result<(), anyhow::Error> {
    for (position, entry) in entries.iter().enumerate() {
        let (title, location, children) = match entry {
            SimpleTocEntry::Leaf(title, location) => (title.as_str(), location, [].as_slice()),
            SimpleTocEntry::Container(title, location, children) => {
                (title.as_str(), location, children.as_slice())
            }
        };

        let (location_kind, location_exact, location_uri) = encode_location(location);
        let id = Uuid7::now();
        let position = position as i64;
        let parent_id_str = parent_id.as_ref().map(ToString::to_string);

        sqlx::query!(
            r#"
            INSERT INTO toc_entries (
                id, book_fingerprint, parent_id, position, title, location_kind, location_exact, location_uri
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            id,
            book_fingerprint,
            parent_id_str,
            position,
            title,
            location_kind,
            location_exact,
            location_uri,
        )
        .execute(&mut **tx)
        .await?;

        if !children.is_empty() {
            Box::pin(insert_toc_entries(tx, book_fingerprint, children, Some(id))).await?;
        }
    }

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn merge_book_authors(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    sqlx::query!(
        "INSERT OR IGNORE INTO book_authors (book_fingerprint, author_id, position)
         SELECT ?, author_id, position
         FROM book_authors
         WHERE book_fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn merge_book_categories(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    sqlx::query!(
        "INSERT OR IGNORE INTO book_categories (book_fingerprint, category_id)
         SELECT ?, category_id
         FROM book_categories
         WHERE book_fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn insert_rekeyed_book(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    sqlx::query(
        r#"
        INSERT INTO books (
            fingerprint, title, subtitle, year, language, publisher,
            series, edition, volume, number, identifier,
            file_kind, file_size, added_at
        )
        SELECT
            ?, title, subtitle, year, language, publisher,
            series, edition, volume, number, identifier,
            file_kind, file_size, added_at
        FROM books WHERE fingerprint = ?
        "#,
    )
    .bind(new_fp)
    .bind(old_fp)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(old_fp = %old_fp, new_fp = %new_fp)))]
async fn move_rekeyed_book_data(
    tx: &mut Transaction<'_, Sqlite>,
    old_fp: &str,
    new_fp: &str,
) -> Result<(), anyhow::Error> {
    sqlx::query!(
        "UPDATE reading_states SET fingerprint = ? WHERE fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query!(
        "UPDATE thumbnails SET fingerprint = ? WHERE fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query!(
        "UPDATE toc_entries SET book_fingerprint = ? WHERE book_fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query!(
        "UPDATE book_authors SET book_fingerprint = ? WHERE book_fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query!(
        "UPDATE book_categories SET book_fingerprint = ? WHERE book_fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query!(
        "UPDATE library_books SET book_fingerprint = ? WHERE book_fingerprint = ?",
        new_fp,
        old_fp,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Ensures the library row exists and returns its id.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool), fields(path = %path, name = %name), ret(level = tracing::Level::TRACE)))]
async fn ensure_library(
    pool: &sqlx::SqlitePool,
    path: &str,
    name: &str,
) -> Result<i64, anyhow::Error> {
    let existing: Option<i64> =
        sqlx::query_scalar!("SELECT id FROM libraries WHERE path = ?", path)
            .fetch_optional(pool)
            .await?
            .flatten();

    if let Some(id) = existing {
        return Ok(id);
    }

    let now = UnixTimestamp::now();
    let result = sqlx::query!(
        "INSERT INTO libraries (path, name, created_at) VALUES (?, ?, ?)",
        path,
        name,
        now
    )
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Imports all books and reading states from a single library directory.
///
/// Loads `.metadata.json` and the `.reading-states/` directory, inserts all
/// entries into the database within a single transaction, then renames the
/// legacy files and removes `.thumbnail-previews/`.
///
/// Returns `(books_imported, reading_states_imported)`.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool), fields(library_id = library_id, path = ?library_path)))]
async fn import_library(
    pool: &sqlx::SqlitePool,
    library_id: i64,
    library_path: &Path,
) -> (usize, usize) {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            error!(path = ?library_path, error = %e, "failed to begin transaction for library import");
            return (0, 0);
        }
    };

    let metadata_path = library_path.join(METADATA_FILENAME);
    let metadata = load_metadata(&metadata_path).await;

    let (books_imported, states_from_metadata, metadata_fps) =
        import_metadata_entries(&mut tx, library_id, metadata).await;

    let reading_states_dir = library_path.join(READING_STATES_DIRNAME);
    let states_from_dir =
        import_orphan_reading_states(&mut tx, library_id, &reading_states_dir, &metadata_fps).await;

    if let Err(e) = tx.commit().await {
        error!(path = ?library_path, error = %e, "failed to commit library import transaction");
        return (0, 0);
    }

    #[cfg(not(feature = "test"))]
    {
        delete_thumbnail_previews(library_path).await;
    }

    (books_imported, states_from_metadata + states_from_dir)
}

/// Imports all entries from a `.metadata.json` file into the database.
///
/// Returns `(books_imported, reading_states_imported, fingerprints_seen)`.
/// The fingerprint set is passed to [`import_orphan_reading_states`] to skip
/// books whose reading state was already written from this file.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx, metadata), fields(library_id = library_id)))]
async fn import_metadata_entries(
    tx: &mut Transaction<'_, Sqlite>,
    library_id: i64,
    metadata: Option<IndexMap<Fp, Info, FxBuildHasher>>,
) -> (usize, usize, HashSet<Fp>) {
    let mut books_imported: usize = 0;
    let mut states_imported: usize = 0;
    let mut seen_fps: HashSet<Fp> = HashSet::new();

    let entries = match metadata {
        Some(e) => e,
        None => return (0, 0, seen_fps),
    };

    for (fp, info) in &entries {
        if let Err(e) = insert_book(tx, library_id, *fp, info).await {
            error!(fp = %fp, error = %e, "failed to insert book from metadata");
            continue;
        }
        books_imported += 1;

        if let Some(reader_info) = info.reader_info.as_ref().or(info.reader.as_ref()) {
            seen_fps.insert(*fp);
            if let Err(e) = insert_reading_state(tx, *fp, reader_info).await {
                error!(fp = %fp, error = %e, "failed to insert reading state from metadata");
            } else {
                states_imported += 1;
            }
        }
    }

    (books_imported, states_imported, seen_fps)
}

/// Imports reading states from `.reading-states/` that are not in `already_imported`.
///
/// The `already_imported` set contains fingerprints whose reading state was
/// already written from `.metadata.json`. Skipping those keeps the migration
/// idempotent and ensures the metadata file's version takes precedence.
///
/// For each fingerprint not yet imported, a stub `books` row is inserted first
/// to satisfy the foreign key constraint. A follow-up migration is responsible
/// for cleaning up any stub rows whose files are no longer on disk.
///
/// Returns the number of reading states imported.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx, already_imported), fields(library_id = library_id, path = ?reading_states_dir)))]
async fn import_orphan_reading_states(
    tx: &mut Transaction<'_, Sqlite>,
    library_id: i64,
    reading_states_dir: &Path,
    already_imported: &HashSet<Fp>,
) -> usize {
    if !reading_states_dir.exists() {
        return 0;
    }

    let mut dir_entries = match fs::read_dir(reading_states_dir).await {
        Ok(d) => d,
        Err(e) => {
            error!(path = ?reading_states_dir, error = %e, "failed to read .reading-states dir");
            return 0;
        }
    };

    let mut states_imported: usize = 0;

    loop {
        let entry = match dir_entries.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => {
                error!(path = ?reading_states_dir, error = %e, "failed to read directory entry");
                break;
            }
        };

        let path = entry.path();

        let fp = match path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| Fp::from_str(s).ok().or_else(|| Fp::from_legacy_str(s).ok()))
        {
            Some(fp) => fp,
            None => {
                warn!(path = ?path, "skipping unrecognised reading-state filename");
                continue;
            }
        };

        if already_imported.contains(&fp) {
            continue;
        }

        let content = match fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => {
                error!(fp = %fp, path = ?path, error = %e, "failed to read reading-state file");
                continue;
            }
        };

        let reader_info: crate::metadata::ReaderInfo = match serde_json::from_str(&content) {
            Ok(r) => r,
            Err(e) => {
                error!(fp = %fp, error = %e, "failed to parse reading-state JSON");
                continue;
            }
        };

        if let Err(e) = ensure_stub_book(tx, library_id, fp).await {
            error!(fp = %fp, error = %e, "failed to insert stub book for orphan reading state, skipping");
            continue;
        }

        if let Err(e) = insert_reading_state(tx, fp, &reader_info).await {
            error!(fp = %fp, error = %e, "failed to insert orphan reading state");
        } else {
            states_imported += 1;
        }
    }

    states_imported
}

/// Inserts a stub `books` row and a `library_books` association for `fp` if
/// they do not already exist.
///
/// The stub has empty strings and zero for the file fields. A follow-up
/// migration is responsible for pruning stub rows whose files are no longer
/// present on disk. `Library::import()` will fill in the real values for files
/// that are still present.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx), fields(library_id = library_id, fp = %fp)))]
async fn ensure_stub_book(
    tx: &mut Transaction<'_, Sqlite>,
    library_id: i64,
    fp: Fp,
) -> Result<(), anyhow::Error> {
    let fp_str = fp.to_string();
    let now = UnixTimestamp::now();

    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO books (fingerprint, file_kind, file_size, added_at)
        VALUES (?, '', 0, ?)
        "#,
        fp_str,
        now,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO library_books (library_id, book_fingerprint, added_to_library_at)
        VALUES (?, ?, ?)
        "#,
        library_id,
        fp_str,
        now,
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Removes `.thumbnail-previews/` from the library directory.
///
/// Thumbnails will be regenerated and stored in the database, so the legacy
/// directory is no longer needed after migration.
#[cfg(not(feature = "test"))]
#[cfg_attr(feature = "tracing", tracing::instrument(fields(path = ?library_path)))]
async fn delete_thumbnail_previews(library_path: &Path) {
    let previews_dir = library_path.join(THUMBNAIL_PREVIEWS_DIRNAME);

    if !previews_dir.exists() {
        return;
    }

    if let Err(e) = fs::remove_dir_all(&previews_dir).await {
        warn!(path = ?previews_dir, error = %e, "failed to delete .thumbnail-previews after import");
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(fields(path = ?path), ret(level = tracing::Level::TRACE)))]
async fn load_metadata(path: &Path) -> Option<IndexMap<Fp, Info, FxBuildHasher>> {
    if !path.exists() {
        return None;
    }

    let content = match fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => {
            error!(path = ?path, error = %e, "failed to read .metadata.json");
            return None;
        }
    };

    match serde_json::from_str(&content) {
        Ok(m) => Some(m),
        Err(e) => {
            error!(path = ?path, error = %e, "failed to parse .metadata.json");
            None
        }
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx, info), fields(library_id = library_id, fp = %fp)))]
async fn insert_book(
    tx: &mut Transaction<'_, Sqlite>,
    library_id: i64,
    fp: Fp,
    info: &Info,
) -> Result<(), anyhow::Error> {
    let book_row = info_to_book_row(fp, info);
    let fp_str = fp.to_string();

    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO books (
            fingerprint, title, subtitle, year, language, publisher,
            series, edition, volume, number, identifier,
            file_kind, file_size, added_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
        book_row.fingerprint,
        book_row.title,
        book_row.subtitle,
        book_row.year,
        book_row.language,
        book_row.publisher,
        book_row.series,
        book_row.edition,
        book_row.volume,
        book_row.number,
        book_row.identifier,
        book_row.file_kind,
        book_row.file_size,
        book_row.added_at,
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO library_books (library_id, book_fingerprint, added_to_library_at, file_path, absolute_path)
        VALUES (?, ?, ?, ?, ?)
        "#,
        library_id,
        fp_str,
        book_row.added_at,
        book_row.file_path,
        book_row.absolute_path,
    )
    .execute(&mut **tx)
    .await?;

    let authors = extract_authors(&info.author);
    for (position, author_name) in authors.iter().enumerate() {
        sqlx::query!(
            r#"INSERT OR IGNORE INTO authors (name) VALUES (?)"#,
            author_name
        )
        .execute(&mut **tx)
        .await?;

        let author_id: i64 =
            sqlx::query_scalar!(r#"SELECT id FROM authors WHERE name = ?"#, author_name)
                .fetch_one(&mut **tx)
                .await?;

        let pos = position as i64;
        sqlx::query!(
            r#"
            INSERT OR IGNORE INTO book_authors (book_fingerprint, author_id, position)
            VALUES (?, ?, ?)
            "#,
            fp_str,
            author_id,
            pos,
        )
        .execute(&mut **tx)
        .await?;
    }

    for category_name in &info.categories {
        sqlx::query!(
            r#"INSERT OR IGNORE INTO categories (name) VALUES (?)"#,
            category_name
        )
        .execute(&mut **tx)
        .await?;

        let category_id: i64 =
            sqlx::query_scalar!(r#"SELECT id FROM categories WHERE name = ?"#, category_name)
                .fetch_one(&mut **tx)
                .await?;

        sqlx::query!(
            r#"
            INSERT OR IGNORE INTO book_categories (book_fingerprint, category_id)
            VALUES (?, ?)
            "#,
            fp_str,
            category_id,
        )
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx, reader_info), fields(fp = %fp)))]
async fn insert_reading_state(
    tx: &mut Transaction<'_, Sqlite>,
    fp: Fp,
    reader_info: &crate::metadata::ReaderInfo,
) -> Result<(), anyhow::Error> {
    let rs = reader_info_to_reading_state_row(fp, reader_info);

    sqlx::query!(
        r#"
        INSERT INTO reading_states (
            fingerprint, opened, current_page, pages_count, finished, dithered,
            zoom_mode, scroll_mode, page_offset_x, page_offset_y, rotation,
            cropping_margins_json, margin_width, screen_margin_width,
            font_family, font_size, text_align, line_height,
            contrast_exponent, contrast_gray,
            page_names_json, bookmarks_json, annotations_json
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(fingerprint) DO NOTHING
        "#,
        rs.fingerprint,
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
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::db::runtime::RUNTIME;
    use crate::document::{SimpleTocEntry, TocLocation};
    use crate::library::db::Db;
    use crate::metadata::{FileInfo, ReaderInfo};
    use chrono::Local;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn create_test_db() -> (Database, Db) {
        let db = Database::new(":memory:").expect("failed to create in-memory database");
        db.migrate().expect("failed to run migrations");
        let libdb = Db::new(&db);

        (db, libdb)
    }

    fn create_info(
        title: &str,
        author: &str,
        categories: &[&str],
        path: &str,
        reader_info: Option<ReaderInfo>,
    ) -> Info {
        Info {
            title: title.to_string(),
            author: author.to_string(),
            categories: categories
                .iter()
                .map(|category| category.to_string())
                .collect::<BTreeSet<_>>(),
            file: FileInfo {
                path: PathBuf::from(path),
                absolute_path: PathBuf::from(path),
                kind: "epub".to_string(),
                size: 1024,
                mtime: None,
            },
            reader_info,
            added: Local::now().naive_local(),
            ..Default::default()
        }
    }

    #[test]
    fn rekey_book_merges_duplicate_content_data() {
        let (db, libdb) = create_test_db();
        let library_a = libdb
            .register_library("/tmp/library-a", "Library A")
            .expect("failed to register library A");
        let library_b = libdb
            .register_library("/tmp/library-b", "Library B")
            .expect("failed to register library B");

        let old_fp = Fp::from_u64(1);
        let new_fp = Fp::from_u64(2);
        let old_reader = ReaderInfo {
            current_page: 42,
            pages_count: 100,
            ..Default::default()
        };
        let old_toc = vec![SimpleTocEntry::Leaf(
            "Chapter 1".to_string(),
            TocLocation::Exact(1),
        )];

        let old_info = create_info(
            "Old Copy",
            "Old Author",
            &["History"],
            "/tmp/library-a/book.epub",
            Some(old_reader.clone()),
        );
        let new_info = create_info("New Copy", "", &[], "/tmp/library-b/book.epub", None);

        libdb
            .insert_book(library_a, old_fp, &old_info)
            .expect("failed to insert old book");
        libdb
            .insert_book(library_b, new_fp, &new_info)
            .expect("failed to insert new book");
        libdb
            .save_toc(old_fp, &old_toc)
            .expect("failed to save old toc");
        libdb
            .save_thumbnail(old_fp, b"old-thumbnail")
            .expect("failed to save old thumbnail");

        RUNTIME.block_on(async {
            rekey_book(db.pool(), &old_fp.to_string(), &new_fp.to_string())
                .await
                .expect("failed to rekey duplicate book");
        });

        let books_a = libdb
            .get_all_books(library_a)
            .expect("failed to load library A books");
        let books_b = libdb
            .get_all_books(library_b)
            .expect("failed to load library B books");

        assert_eq!(books_a.len(), 1);
        assert_eq!(books_b.len(), 1);
        assert_eq!(books_a[0].fp, Some(new_fp));
        assert_eq!(books_b[0].fp, Some(new_fp));

        let merged = &books_a[0];
        let merged_reader = merged
            .reader_info
            .as_ref()
            .expect("reading state should be preserved");

        assert_eq!(merged.author, "Old Author");
        assert!(merged.categories.contains("History"));
        assert_eq!(merged_reader.current_page, old_reader.current_page);
        assert_eq!(merged_reader.pages_count, old_reader.pages_count);
        assert!(matches!(
            merged.toc.as_ref(),
            Some(toc) if matches!(toc.first(), Some(SimpleTocEntry::Leaf(title, TocLocation::Exact(1))) if title == "Chapter 1")
        ));
        assert_eq!(
            libdb
                .get_thumbnail(new_fp)
                .expect("failed to read thumbnail"),
            Some(b"old-thumbnail".to_vec())
        );
        assert_eq!(
            libdb
                .get_thumbnail(old_fp)
                .expect("failed to read old thumbnail"),
            None
        );
    }

    #[test]
    fn rekey_book_keeps_existing_duplicate_data() {
        let (db, libdb) = create_test_db();
        let library_a = libdb
            .register_library("/tmp/library-c", "Library C")
            .expect("failed to register library C");
        let library_b = libdb
            .register_library("/tmp/library-d", "Library D")
            .expect("failed to register library D");

        let old_fp = Fp::from_u64(3);
        let new_fp = Fp::from_u64(4);
        let old_toc = vec![SimpleTocEntry::Leaf(
            "Old Chapter".to_string(),
            TocLocation::Exact(1),
        )];
        let new_toc = vec![SimpleTocEntry::Leaf(
            "New Chapter".to_string(),
            TocLocation::Exact(2),
        )];

        let old_info = create_info(
            "Old Copy",
            "Old Author",
            &["History"],
            "/tmp/library-c/book.epub",
            Some(ReaderInfo {
                current_page: 12,
                pages_count: 100,
                ..Default::default()
            }),
        );
        let new_info = create_info(
            "New Copy",
            "",
            &[],
            "/tmp/library-d/book.epub",
            Some(ReaderInfo {
                current_page: 88,
                pages_count: 200,
                ..Default::default()
            }),
        );

        libdb
            .insert_book(library_a, old_fp, &old_info)
            .expect("failed to insert old book");
        libdb
            .insert_book(library_b, new_fp, &new_info)
            .expect("failed to insert new book");
        libdb
            .save_toc(old_fp, &old_toc)
            .expect("failed to save old toc");
        libdb
            .save_toc(new_fp, &new_toc)
            .expect("failed to save new toc");
        libdb
            .save_thumbnail(old_fp, b"old-thumbnail")
            .expect("failed to save old thumbnail");
        libdb
            .save_thumbnail(new_fp, b"new-thumbnail")
            .expect("failed to save new thumbnail");

        RUNTIME.block_on(async {
            rekey_book(db.pool(), &old_fp.to_string(), &new_fp.to_string())
                .await
                .expect("failed to rekey duplicate book");
        });

        let merged = libdb
            .get_all_books(library_a)
            .expect("failed to load merged books")
            .into_iter()
            .next()
            .expect("merged book should exist");
        let merged_reader = merged
            .reader_info
            .as_ref()
            .expect("reading state should exist");

        assert_eq!(merged_reader.current_page, 88);
        assert!(matches!(
            merged.toc.as_ref(),
            Some(toc) if matches!(toc.first(), Some(SimpleTocEntry::Leaf(title, TocLocation::Exact(2))) if title == "New Chapter")
        ));
        assert_eq!(
            libdb
                .get_thumbnail(new_fp)
                .expect("failed to read thumbnail"),
            Some(b"new-thumbnail".to_vec())
        );
    }

    #[test]
    fn rehash_fingerprints_canonicalizes_unrekeyed_legacy_fingerprints() {
        let (db, libdb) = create_test_db();
        let library_id = libdb
            .register_library("/tmp/library-legacy", "Legacy Library")
            .expect("failed to register legacy library");
        let legacy_fp = "0000000000000001";
        let legacy_fp_value =
            Fp::from_legacy_str(legacy_fp).expect("legacy fingerprint should parse");
        let canonical_fp = legacy_fp_value.to_string();

        RUNTIME.block_on(async {
            let mut tx = db
                .pool()
                .begin()
                .await
                .expect("failed to begin legacy insert transaction");

            ensure_stub_book(&mut tx, library_id, legacy_fp_value)
                .await
                .expect("failed to insert legacy book");

            tx.commit()
                .await
                .expect("failed to commit legacy insert transaction");

            rehash_fingerprints(db.pool())
                .await
                .expect("failed to run rehash migration");
        });

        let books = libdb
            .get_all_books(library_id)
            .expect("canonicalized legacy books should load");

        assert_eq!(books.len(), 1);
        assert_eq!(
            books[0].fp.map(|fp| fp.to_string()).as_deref(),
            Some(canonical_fp.as_str())
        );

        RUNTIME.block_on(async {
            let old_row = sqlx::query_scalar!(
                "SELECT fingerprint FROM books WHERE fingerprint = ?",
                legacy_fp
            )
            .fetch_optional(db.pool())
            .await
            .expect("failed to query old fingerprint");
            let new_row = sqlx::query_scalar!(
                "SELECT fingerprint FROM books WHERE fingerprint = ?",
                canonical_fp
            )
            .fetch_optional(db.pool())
            .await
            .expect("failed to query canonical fingerprint");

            assert!(old_row.is_none());
            assert_eq!(new_row.as_deref(), Some(canonical_fp.as_str()));
        });
    }

    #[test]
    fn rehash_fingerprints_reads_absolute_path_from_library_books() {
        let temp = tempdir().expect("failed to create temp dir");
        let library_root = temp.path().join("library");
        std::fs::create_dir(&library_root).expect("failed to create library root");
        let book_path = library_root.join("book.epub");
        std::fs::write(&book_path, b"rehash me").expect("failed to write book file");

        let (db, libdb) = create_test_db();
        let library_id = libdb
            .register_library(library_root.to_string_lossy().as_ref(), "Rehash Library")
            .expect("failed to register library");
        let legacy_fp = "00000000000000aa";
        let expected_fp = book_path.fingerprint().expect("failed to fingerprint file");
        let now = UnixTimestamp::now();

        RUNTIME.block_on(async {
            sqlx::query(
                r#"
                INSERT INTO books (
                    fingerprint, title, subtitle, year, language, publisher,
                    series, edition, volume, number, identifier,
                    file_kind, file_size, added_at
                ) VALUES (?, ?, '', '', '', '', '', '', '', '', '', ?, ?, ?)
                "#,
            )
            .bind(legacy_fp)
            .bind("Legacy Book")
            .bind("epub")
            .bind(9_i64)
            .bind(now)
            .execute(db.pool())
            .await
            .expect("failed to insert legacy book");

            sqlx::query(
                r#"
                INSERT INTO library_books (
                    library_id, book_fingerprint, added_to_library_at, file_path, absolute_path
                ) VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(library_id)
            .bind(legacy_fp)
            .bind(now)
            .bind("book.epub")
            .bind(book_path.to_string_lossy().as_ref())
            .execute(db.pool())
            .await
            .expect("failed to insert library book row");

            rehash_fingerprints(db.pool())
                .await
                .expect("failed to run rehash migration");
        });

        let books = libdb
            .get_all_books(library_id)
            .expect("rehash results should load");

        assert_eq!(books.len(), 1);
        assert_eq!(books[0].fp, Some(expected_fp));
        assert_eq!(books[0].file.absolute_path, book_path);

        RUNTIME.block_on(async {
            let expected_fp_str = expected_fp.to_string();
            let old_row = sqlx::query_scalar!(
                "SELECT fingerprint FROM books WHERE fingerprint = ?",
                legacy_fp
            )
            .fetch_optional(db.pool())
            .await
            .expect("failed to query legacy row");
            let new_row = sqlx::query_scalar!(
                "SELECT fingerprint FROM books WHERE fingerprint = ?",
                expected_fp_str
            )
            .fetch_optional(db.pool())
            .await
            .expect("failed to query rehashed row");

            assert!(old_row.is_none());
            assert_eq!(new_row.as_deref(), Some(expected_fp_str.as_str()));
        });
    }
}
