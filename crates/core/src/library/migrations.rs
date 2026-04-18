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

use crate::db::types::UnixTimestamp;
use crate::helpers::Fp;
use crate::library::db::conversion::{
    extract_authors, info_to_book_row, reader_info_to_reading_state_row,
};
#[cfg(not(feature = "test"))]
use crate::library::THUMBNAIL_PREVIEWS_DIRNAME;
use crate::library::{METADATA_FILENAME, READING_STATES_DIRNAME};
use crate::metadata::Info;
use crate::settings::versioned::SettingsManager;
use crate::version::get_current_version;
use fxhash::FxBuildHasher;
use indexmap::IndexMap;
use sqlx::{Sqlite, Transaction};
use std::collections::HashSet;
use std::path::Path;
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
        let settings = SettingsManager::new(get_current_version()).load();

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

/// Ensures the library row exists and returns its id.
#[cfg_attr(feature = "otel", tracing::instrument(skip(pool), fields(path = %path, name = %name), ret(level = tracing::Level::TRACE)))]
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
#[cfg_attr(feature = "otel", tracing::instrument(skip(pool), fields(library_id = library_id, path = ?library_path)))]
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
        mark_library_imported(library_path).await;
        delete_thumbnail_previews(library_path).await;
    }

    (books_imported, states_from_metadata + states_from_dir)
}

/// Imports all entries from a `.metadata.json` file into the database.
///
/// Returns `(books_imported, reading_states_imported, fingerprints_seen)`.
/// The fingerprint set is passed to [`import_orphan_reading_states`] to skip
/// books whose reading state was already written from this file.
#[cfg_attr(feature = "otel", tracing::instrument(skip(tx, metadata), fields(library_id = library_id)))]
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
#[cfg_attr(feature = "otel", tracing::instrument(skip(tx, already_imported), fields(library_id = library_id, path = ?reading_states_dir)))]
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
            .and_then(|s| Fp::from_str(s).ok())
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
#[cfg_attr(feature = "otel", tracing::instrument(skip(tx), fields(library_id = library_id, fp = %fp)))]
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

/// Renames `.metadata.json` and `.reading-states/` to their `.imported` suffixed
/// equivalents so that subsequent runs of the migration skip this library.
#[cfg(not(feature = "test"))]
#[cfg_attr(feature = "otel", tracing::instrument(fields(path = ?library_path)))]
async fn mark_library_imported(library_path: &Path) {
    let metadata_src = library_path.join(METADATA_FILENAME);
    let metadata_dst = library_path.join(format!("{}.imported", METADATA_FILENAME));

    if metadata_src.exists() {
        if let Err(e) = fs::rename(&metadata_src, &metadata_dst).await {
            warn!(path = ?metadata_src, error = %e, "failed to rename .metadata.json after import");
        }
    }

    let states_src = library_path.join(READING_STATES_DIRNAME);
    let states_dst = library_path.join(format!("{}.imported", READING_STATES_DIRNAME));

    if states_src.exists() {
        if let Err(e) = fs::rename(&states_src, &states_dst).await {
            warn!(path = ?states_src, error = %e, "failed to rename .reading-states after import");
        }
    }
}

/// Removes `.thumbnail-previews/` from the library directory.
///
/// Thumbnails will be regenerated and stored in the database, so the legacy
/// directory is no longer needed after migration.
#[cfg(not(feature = "test"))]
#[cfg_attr(feature = "otel", tracing::instrument(fields(path = ?library_path)))]
async fn delete_thumbnail_previews(library_path: &Path) {
    let previews_dir = library_path.join(THUMBNAIL_PREVIEWS_DIRNAME);

    if !previews_dir.exists() {
        return;
    }

    if let Err(e) = fs::remove_dir_all(&previews_dir).await {
        warn!(path = ?previews_dir, error = %e, "failed to delete .thumbnail-previews after import");
    }
}

#[cfg_attr(feature = "otel", tracing::instrument(fields(path = ?path), ret(level = tracing::Level::TRACE)))]
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

#[cfg_attr(feature = "otel", tracing::instrument(skip(tx, info), fields(library_id = library_id, fp = %fp)))]
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

#[cfg_attr(feature = "otel", tracing::instrument(skip(tx, reader_info), fields(fp = %fp)))]
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
