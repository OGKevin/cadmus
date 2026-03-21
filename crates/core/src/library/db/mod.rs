pub mod conversion;
pub mod models;

use crate::db::runtime::RUNTIME;
use crate::db::types::{OptionalUuid7, UnixTimestamp, Uuid7};
use crate::db::Database;
use crate::document::SimpleTocEntry;
use crate::geom::Point;
use crate::helpers::Fp;
use crate::metadata::{
    CroppingMargins, FileInfo, Info, ReaderInfo, ScrollMode, TextAlign, ZoomMode,
};
use anyhow::Error;
use conversion::{
    extract_authors, info_to_book_row, reader_info_to_reading_state_row, rows_to_toc_entries,
};
use models::TocEntryRow;
use sqlx::sqlite::SqlitePool;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(path = %path, name = %name)))]
    pub fn register_library(&self, path: &str, name: &str) -> Result<i64, Error> {
        tracing::debug!(path = %path, name = %name, "registering library");

        RUNTIME.block_on(async {
            let now = UnixTimestamp::now();

            let result = sqlx::query!(
                r#"
                INSERT INTO libraries (path, name, created_at)
                VALUES (?, ?, ?)
                "#,
                path,
                name,
                now
            )
            .execute(&self.pool)
            .await?;

            let library_id = result.last_insert_rowid();
            tracing::info!(library_id, path = %path, name = %name, "library registered");
            Ok(library_id)
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(path = %path)))]
    pub fn get_library_by_path(&self, path: &str) -> Result<Option<i64>, Error> {
        tracing::debug!(path = %path, "looking up library by path");

        RUNTIME.block_on(async {
            let id: Option<Option<i64>> =
                sqlx::query_scalar!(r#"SELECT id FROM libraries WHERE path = ?"#, path)
                    .fetch_optional(&self.pool)
                    .await?;

            Ok(id.flatten())
        })
    }

    #[inline]
    fn parse_zoom_mode(json: Option<&String>) -> Option<ZoomMode> {
        match json {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse zoom_mode JSON field");
                    None
                }
            },
            None => None,
        }
    }

    #[inline]
    fn parse_scroll_mode(json: Option<&String>) -> Option<ScrollMode> {
        match json {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse scroll_mode JSON field");
                    None
                }
            },
            None => None,
        }
    }

    #[inline]
    fn parse_text_align(json: Option<&String>) -> Option<TextAlign> {
        match json {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse text_align JSON field");
                    None
                }
            },
            None => None,
        }
    }

    #[inline]
    fn parse_cropping_margins(json: Option<&String>) -> Option<CroppingMargins> {
        match json {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse cropping_margins JSON field");
                    None
                }
            },
            None => None,
        }
    }

    #[inline]
    fn parse_page_names(json: Option<&String>) -> BTreeMap<usize, String> {
        match json {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse page_names JSON field");
                    BTreeMap::default()
                }
            },
            None => BTreeMap::default(),
        }
    }

    #[inline]
    fn parse_bookmarks(json: Option<&String>) -> BTreeSet<usize> {
        match json {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse bookmarks JSON field");
                    BTreeSet::default()
                }
            },
            None => BTreeSet::default(),
        }
    }

    #[inline]
    fn parse_annotations(json: Option<&String>) -> Vec<crate::metadata::Annotation> {
        match json {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse annotations JSON field");
                    Vec::new()
                }
            },
            None => Vec::new(),
        }
    }

    #[inline]
    fn parse_page_offset(x: Option<i64>, y: Option<i64>) -> Option<Point> {
        match (x, y) {
            (Some(x_val), Some(y_val)) => Some(Point::new(x_val as i32, y_val as i32)),
            _ => None,
        }
    }

    #[inline]
    fn extract_authors(authors: Option<String>) -> String {
        authors
            .map(|s| s.split(',').collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    }

    #[inline]
    fn extract_categories(categories: Option<String>) -> BTreeSet<String> {
        categories
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(conn, entries), fields(book_fingerprint = %book_fingerprint, parent_id = ?parent_id)))]
    async fn insert_toc_entries(
        conn: &mut sqlx::SqliteConnection,
        book_fingerprint: &str,
        entries: &[SimpleTocEntry],
        parent_id: Option<Uuid7>,
    ) -> Result<(), Error> {
        for (position, entry) in entries.iter().enumerate() {
            let (title, location, children) = match entry {
                SimpleTocEntry::Leaf(t, loc) => (t.as_str(), loc, [].as_slice()),
                SimpleTocEntry::Container(t, loc, ch) => (t.as_str(), loc, ch.as_slice()),
            };

            let (location_kind, location_exact, location_uri) =
                conversion::encode_location(location);
            let pos = position as i64;
            let id = Uuid7::now();
            let parent_id_str = parent_id.as_ref().map(|p| p.to_string());

            sqlx::query!(
                r#"
                INSERT INTO toc_entries (id, book_fingerprint, parent_id, position, title, location_kind, location_exact, location_uri)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
                id,
                book_fingerprint,
                parent_id_str,
                pos,
                title,
                location_kind,
                location_exact,
                location_uri,
            )
            .execute(&mut *conn)
            .await?;

            if !children.is_empty() {
                Box::pin(Self::insert_toc_entries(
                    conn,
                    book_fingerprint,
                    children,
                    Some(id),
                ))
                .await?;
            }
        }

        Ok(())
    }

    async fn fetch_all_toc_entries(
        pool: &SqlitePool,
        library_id: i64,
    ) -> Result<HashMap<String, Vec<TocEntryRow>>, Error> {
        let toc_rows: Vec<TocEntryRow> = sqlx::query_as!(
            TocEntryRow,
            r#"
            SELECT
                te.book_fingerprint,
                te.id                as "id: Uuid7",
                te.parent_id         as "parent_id!: OptionalUuid7",
                te.position,
                te.title,
                te.location_kind,
                te.location_exact,
                te.location_uri
            FROM toc_entries te
            INNER JOIN library_books lb ON lb.book_fingerprint = te.book_fingerprint
            WHERE lb.library_id = ?
            ORDER BY te.book_fingerprint, te.id ASC
            "#,
            library_id
        )
        .fetch_all(pool)
        .await?;

        let mut map: HashMap<String, Vec<TocEntryRow>> = HashMap::new();

        for row in toc_rows {
            map.entry(row.book_fingerprint.clone())
                .or_default()
                .push(row);
        }

        Ok(map)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(library_id)))]
    pub fn get_all_books(&self, library_id: i64) -> Result<Vec<(Fp, Info)>, Error> {
        tracing::debug!(library_id, "fetching all books from database");

        RUNTIME.block_on(async {
            let book_rows = sqlx::query!(
                r#"
                SELECT
                    fingerprint,
                    title,
                    subtitle,
                    year,
                    language,
                    publisher,
                    series,
                    edition,
                    volume,
                    number,
                    identifier,
                    file_path,
                    absolute_path,
                    file_kind,
                    file_size,
                    added_at              as "added_at: UnixTimestamp",
                    opened                as "opened?: UnixTimestamp",
                    current_page          as "current_page?: i64",
                    pages_count           as "pages_count?: i64",
                    finished              as "finished?: i64",
                    dithered              as "dithered?: i64",
                    zoom_mode             as "zoom_mode?: String",
                    scroll_mode           as "scroll_mode?: String",
                    page_offset_x         as "page_offset_x?: i64",
                    page_offset_y         as "page_offset_y?: i64",
                    rotation              as "rotation?: i64",
                    cropping_margins_json as "cropping_margins_json?: String",
                    margin_width          as "margin_width?: i64",
                    screen_margin_width   as "screen_margin_width?: i64",
                    font_family           as "font_family?: String",
                    font_size             as "font_size?: f64",
                    text_align            as "text_align?: String",
                    line_height           as "line_height?: f64",
                    contrast_exponent     as "contrast_exponent?: f64",
                    contrast_gray         as "contrast_gray?: f64",
                    page_names_json       as "page_names_json?: String",
                    bookmarks_json        as "bookmarks_json?: String",
                    annotations_json      as "annotations_json?: String",
                    authors               as "authors?: String",
                    categories            as "categories?: String"
                FROM library_books_full_info
                WHERE library_id = ?
                ORDER BY added_at DESC
                "#,
                library_id
            )
            .fetch_all(&self.pool)
            .await?;

            let mut toc_by_fingerprint =
                Self::fetch_all_toc_entries(&self.pool, library_id).await?;

            let mut result = Vec::new();

            for row in book_rows {
                let fp = Fp::from_str(&row.fingerprint)?;

                let toc = toc_by_fingerprint
                    .remove(&row.fingerprint)
                    .map(|rows| rows_to_toc_entries(&rows))
                    .transpose()?;

                let mut info = Info {
                    title: row.title,
                    subtitle: row.subtitle,
                    author: Self::extract_authors(row.authors),
                    year: row.year,
                    language: row.language,
                    publisher: row.publisher,
                    series: row.series,
                    edition: row.edition,
                    volume: row.volume,
                    number: row.number,
                    identifier: row.identifier,
                    categories: Self::extract_categories(row.categories),
                    file: FileInfo {
                        path: PathBuf::from(&row.file_path),
                        absolute_path: PathBuf::from(&row.absolute_path),
                        kind: row.file_kind,
                        size: row.file_size as u64,
                    },
                    reader: None,
                    reader_info: None,
                    toc,
                    added: row.added_at.into(),
                };

                if let Some(opened_ts) = row.opened {
                    let reader_info = ReaderInfo {
                        opened: opened_ts.into(),
                        current_page: row.current_page.unwrap_or(0) as usize,
                        pages_count: row.pages_count.unwrap_or(0) as usize,
                        finished: row.finished.unwrap_or(0) == 1,
                        dithered: row.dithered.unwrap_or(0) == 1,
                        zoom_mode: Self::parse_zoom_mode(row.zoom_mode.as_ref()),
                        scroll_mode: Self::parse_scroll_mode(row.scroll_mode.as_ref()),
                        page_offset: Self::parse_page_offset(row.page_offset_x, row.page_offset_y),
                        rotation: row.rotation.map(|r| r as i8),
                        cropping_margins: Self::parse_cropping_margins(
                            row.cropping_margins_json.as_ref(),
                        ),
                        margin_width: row.margin_width.map(|m| m as i32),
                        screen_margin_width: row.screen_margin_width.map(|m| m as i32),
                        font_family: row.font_family.clone(),
                        font_size: row.font_size.map(|f| f as f32),
                        text_align: Self::parse_text_align(row.text_align.as_ref()),
                        line_height: row.line_height.map(|l| l as f32),
                        contrast_exponent: row.contrast_exponent.map(|c| c as f32),
                        contrast_gray: row.contrast_gray.map(|c| c as f32),
                        page_names: Self::parse_page_names(row.page_names_json.as_ref()),
                        bookmarks: Self::parse_bookmarks(row.bookmarks_json.as_ref()),
                        annotations: Self::parse_annotations(row.annotations_json.as_ref()),
                    };
                    info.reader = Some(reader_info.clone());
                    info.reader_info = Some(reader_info);
                }

                result.push((fp, info));
            }

            tracing::debug!(library_id, count = result.len(), "fetched all books");
            Ok(result)
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, info), fields(fp = %fp, library_id)))]
    pub fn insert_book(&self, library_id: i64, fp: Fp, info: &Info) -> Result<(), Error> {
        tracing::debug!(fp = %fp, library_id, "inserting book into database");
        let fp_str = fp.to_string();

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            let book_row = info_to_book_row(fp, info);

            sqlx::query!(
                r#"
                INSERT OR IGNORE INTO books (
                    fingerprint, title, subtitle, year, language, publisher,
                    series, edition, volume, number, identifier,
                    absolute_path, file_kind, file_size, added_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                book_row.absolute_path,
                book_row.file_kind,
                book_row.file_size,
                book_row.added_at,
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                r#"
                INSERT OR IGNORE INTO library_books (library_id, book_fingerprint, added_to_library_at, file_path)
                VALUES (?, ?, ?, ?)
                "#,
                library_id,
                fp_str,
                book_row.added_at,
                book_row.file_path,
            )
            .execute(&mut *tx)
            .await?;

            let authors = extract_authors(&info.author);
            for (position, author_name) in authors.iter().enumerate() {
                sqlx::query!(
                    r#"INSERT OR IGNORE INTO authors (name) VALUES (?)"#,
                    author_name
                )
                .execute(&mut *tx)
                .await?;

                let author_id: i64 = sqlx::query_scalar!(
                    r#"SELECT id FROM authors WHERE name = ?"#,
                    author_name
                )
                .fetch_one(&mut *tx)
                .await?;

                let pos = position as i64;
                sqlx::query!(
                    r#"
                    INSERT OR IGNORE INTO book_authors (book_fingerprint, author_id, position)
                    VALUES (?, ?, ?)
                    "#,
                    fp_str,
                    author_id,
                    pos
                )
                .execute(&mut *tx)
                .await?;
            }

            for category_name in &info.categories {
                sqlx::query!(
                    r#"INSERT OR IGNORE INTO categories (name) VALUES (?)"#,
                    category_name
                )
                .execute(&mut *tx)
                .await?;

                let category_id: i64 = sqlx::query_scalar!(
                    r#"SELECT id FROM categories WHERE name = ?"#,
                    category_name
                )
                .fetch_one(&mut *tx)
                .await?;

                sqlx::query!(
                    r#"
                    INSERT OR IGNORE INTO book_categories (book_fingerprint, category_id)
                    VALUES (?, ?)
                    "#,
                    fp_str,
                    category_id
                )
                .execute(&mut *tx)
                .await?;
            }

            if let Some(reader_info) = &info.reader_info {
                let rs_row = reader_info_to_reading_state_row(fp, reader_info);

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
                    "#,
                    rs_row.fingerprint,
                    rs_row.opened,
                    rs_row.current_page,
                    rs_row.pages_count,
                    rs_row.finished,
                    rs_row.dithered,
                    rs_row.zoom_mode,
                    rs_row.scroll_mode,
                    rs_row.page_offset_x,
                    rs_row.page_offset_y,
                    rs_row.rotation,
                    rs_row.cropping_margins_json,
                    rs_row.margin_width,
                    rs_row.screen_margin_width,
                    rs_row.font_family,
                    rs_row.font_size,
                    rs_row.text_align,
                    rs_row.line_height,
                    rs_row.contrast_exponent,
                    rs_row.contrast_gray,
                    rs_row.page_names_json,
                    rs_row.bookmarks_json,
                    rs_row.annotations_json,
                )
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;

            tracing::debug!(fp = %fp, "book insert complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, info), fields(fp = %fp, library_id)))]
    pub fn update_book(&self, library_id: i64, fp: Fp, info: &Info) -> Result<(), Error> {
        tracing::debug!(fp = %fp, library_id, "updating book in database");
        let fp_str = fp.to_string();

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            let book_row = info_to_book_row(fp, info);

            sqlx::query!(
                r#"
                UPDATE books SET
                    title = ?, subtitle = ?, year = ?, language = ?, publisher = ?,
                    series = ?, edition = ?, volume = ?, number = ?, identifier = ?,
                    absolute_path = ?, file_kind = ?, file_size = ?, added_at = ?
                WHERE fingerprint = ?
                "#,
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
                book_row.absolute_path,
                book_row.file_kind,
                book_row.file_size,
                book_row.added_at,
                fp_str,
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                r#"
                UPDATE library_books SET file_path = ?
                WHERE library_id = ? AND book_fingerprint = ?
                "#,
                book_row.file_path,
                library_id,
                fp_str,
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                r#"DELETE FROM book_authors WHERE book_fingerprint = ?"#,
                fp_str
            )
            .execute(&mut *tx)
            .await?;

            let authors = extract_authors(&info.author);
            for (position, author_name) in authors.iter().enumerate() {
                sqlx::query!(
                    r#"INSERT OR IGNORE INTO authors (name) VALUES (?)"#,
                    author_name
                )
                .execute(&mut *tx)
                .await?;

                let author_id: i64 =
                    sqlx::query_scalar!(r#"SELECT id FROM authors WHERE name = ?"#, author_name)
                        .fetch_one(&mut *tx)
                        .await?;

                let pos = position as i64;
                sqlx::query!(
                    r#"
                    INSERT INTO book_authors (book_fingerprint, author_id, position)
                    VALUES (?, ?, ?)
                    "#,
                    fp_str,
                    author_id,
                    pos
                )
                .execute(&mut *tx)
                .await?;
            }

            sqlx::query!(
                r#"DELETE FROM book_categories WHERE book_fingerprint = ?"#,
                fp_str
            )
            .execute(&mut *tx)
            .await?;

            for category_name in &info.categories {
                sqlx::query!(
                    r#"INSERT OR IGNORE INTO categories (name) VALUES (?)"#,
                    category_name
                )
                .execute(&mut *tx)
                .await?;

                let category_id: i64 = sqlx::query_scalar!(
                    r#"SELECT id FROM categories WHERE name = ?"#,
                    category_name
                )
                .fetch_one(&mut *tx)
                .await?;

                sqlx::query!(
                    r#"
                    INSERT INTO book_categories (book_fingerprint, category_id)
                    VALUES (?, ?)
                    "#,
                    fp_str,
                    category_id
                )
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;

            tracing::debug!(fp = %fp, "book update complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(fp = %fp)))]
    pub fn delete_reading_state(&self, fp: Fp) -> Result<(), Error> {
        tracing::debug!(fp = %fp, "deleting reading state from database");

        RUNTIME.block_on(async {
            let fp_str = fp.to_string();

            sqlx::query!(
                r#"DELETE FROM reading_states WHERE fingerprint = ?"#,
                fp_str
            )
            .execute(&self.pool)
            .await?;

            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(fp = %fp, library_id)))]
    pub fn delete_book(&self, library_id: i64, fp: Fp) -> Result<(), Error> {
        tracing::debug!(fp = %fp, library_id, "deleting book from library");

        RUNTIME.block_on(async {
            let fp_str = fp.to_string();
            let mut tx = self.pool.begin().await?;

            sqlx::query!(
                r#"DELETE FROM library_books WHERE library_id = ? AND book_fingerprint = ?"#,
                library_id,
                fp_str
            )
            .execute(&mut *tx)
            .await?;

            let remaining: i64 = sqlx::query_scalar!(
                r#"SELECT COUNT(*) FROM library_books WHERE book_fingerprint = ?"#,
                fp_str
            )
            .fetch_one(&mut *tx)
            .await?;

            if remaining == 0 {
                tracing::debug!(fp = %fp, "book not in any library, deleting completely");
                sqlx::query!(r#"DELETE FROM books WHERE fingerprint = ?"#, fp_str)
                    .execute(&mut *tx)
                    .await?;
            }

            tx.commit().await?;

            tracing::debug!(fp = %fp, library_id, "book delete complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(fp = %fp)))]
    pub fn get_thumbnail(&self, fp: Fp) -> Result<Option<Vec<u8>>, Error> {
        tracing::debug!(fp = %fp, "fetching thumbnail from database");
        let fp_str = fp.to_string();

        RUNTIME.block_on(async {
            sqlx::query_scalar!(
                "SELECT thumbnail_data FROM thumbnails WHERE fingerprint = ?",
                fp_str
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, data), fields(fp = %fp, size = data.len())))]
    pub fn save_thumbnail(&self, fp: Fp, data: &[u8]) -> Result<(), Error> {
        tracing::debug!(fp = %fp, size = data.len(), "saving thumbnail to database");
        let fp_str = fp.to_string();

        RUNTIME.block_on(async {
            sqlx::query!(
                r#"
                INSERT INTO thumbnails (fingerprint, thumbnail_data)
                VALUES (?, ?)
                ON CONFLICT(fingerprint) DO UPDATE SET
                    thumbnail_data = excluded.thumbnail_data
                "#,
                fp_str,
                data,
            )
            .execute(&self.pool)
            .await?;

            tracing::debug!(fp = %fp, "thumbnail save complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(fp = %fp)))]
    pub fn delete_thumbnail(&self, fp: Fp) -> Result<(), Error> {
        tracing::debug!(fp = %fp, "deleting thumbnail from database");
        let fp_str = fp.to_string();

        RUNTIME.block_on(async {
            sqlx::query!("DELETE FROM thumbnails WHERE fingerprint = ?", fp_str)
                .execute(&self.pool)
                .await?;

            tracing::debug!(fp = %fp, "thumbnail delete complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(from = %from_fp, to = %to_fp)))]
    pub fn move_thumbnail(&self, from_fp: Fp, to_fp: Fp) -> Result<(), Error> {
        tracing::debug!(from = %from_fp, to = %to_fp, "moving thumbnail in database");
        let from_fp_str = from_fp.to_string();
        let to_fp_str = to_fp.to_string();

        RUNTIME.block_on(async {
            sqlx::query!(
                r#"
                UPDATE thumbnails
                SET fingerprint = ?
                WHERE fingerprint = ?
                "#,
                to_fp_str,
                from_fp_str
            )
            .execute(&self.pool)
            .await?;

            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, reader_info), fields(fp = %fp)))]
    pub fn save_reading_state(&self, fp: Fp, reader_info: &ReaderInfo) -> Result<(), Error> {
        tracing::debug!(fp = %fp, "saving reading state to database");

        RUNTIME.block_on(async {
            let rs_row = reader_info_to_reading_state_row(fp, reader_info);

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
                ON CONFLICT(fingerprint) DO UPDATE SET
                    opened = excluded.opened,
                    current_page = excluded.current_page,
                    pages_count = excluded.pages_count,
                    finished = excluded.finished,
                    dithered = excluded.dithered,
                    zoom_mode = excluded.zoom_mode,
                    scroll_mode = excluded.scroll_mode,
                    page_offset_x = excluded.page_offset_x,
                    page_offset_y = excluded.page_offset_y,
                    rotation = excluded.rotation,
                    cropping_margins_json = excluded.cropping_margins_json,
                    margin_width = excluded.margin_width,
                    screen_margin_width = excluded.screen_margin_width,
                    font_family = excluded.font_family,
                    font_size = excluded.font_size,
                    text_align = excluded.text_align,
                    line_height = excluded.line_height,
                    contrast_exponent = excluded.contrast_exponent,
                    contrast_gray = excluded.contrast_gray,
                    page_names_json = excluded.page_names_json,
                    bookmarks_json = excluded.bookmarks_json,
                    annotations_json = excluded.annotations_json
                "#,
                rs_row.fingerprint,
                rs_row.opened,
                rs_row.current_page,
                rs_row.pages_count,
                rs_row.finished,
                rs_row.dithered,
                rs_row.zoom_mode,
                rs_row.scroll_mode,
                rs_row.page_offset_x,
                rs_row.page_offset_y,
                rs_row.rotation,
                rs_row.cropping_margins_json,
                rs_row.margin_width,
                rs_row.screen_margin_width,
                rs_row.font_family,
                rs_row.font_size,
                rs_row.text_align,
                rs_row.line_height,
                rs_row.contrast_exponent,
                rs_row.contrast_gray,
                rs_row.page_names_json,
                rs_row.bookmarks_json,
                rs_row.annotations_json,
            )
            .execute(&self.pool)
            .await?;

            tracing::debug!(fp = %fp, "reading state save complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, toc), fields(fp = %fp, entry_count = toc.len())))]
    pub fn save_toc(&self, fp: Fp, toc: &[SimpleTocEntry]) -> Result<(), Error> {
        if toc.is_empty() {
            return Ok(());
        }

        tracing::debug!(fp = %fp, entry_count = toc.len(), "saving TOC to database");
        let fp_str = fp.to_string();

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            sqlx::query!("DELETE FROM toc_entries WHERE book_fingerprint = ?", fp_str)
                .execute(&mut *tx)
                .await?;

            Self::insert_toc_entries(&mut tx, &fp_str, toc, None).await?;

            tx.commit().await?;

            tracing::debug!(fp = %fp, "TOC save complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, books), fields(library_id, count = books.len())))]
    pub fn batch_insert_books(&self, library_id: i64, books: &[(Fp, &Info)]) -> Result<(), Error> {
        if books.is_empty() {
            return Ok(());
        }

        tracing::debug!(library_id, count = books.len(), "batch inserting books");

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            for (fp, info) in books {
                let fp_str = fp.to_string();
                let book_row = info_to_book_row(*fp, info);

                sqlx::query!(
                    r#"
                    INSERT OR IGNORE INTO books (
                        fingerprint, title, subtitle, year, language, publisher,
                        series, edition, volume, number, identifier,
                        absolute_path, file_kind, file_size, added_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                    book_row.absolute_path,
                    book_row.file_kind,
                    book_row.file_size,
                    book_row.added_at,
                )
                .execute(&mut *tx)
                .await?;

                sqlx::query!(
                    r#"
                    INSERT OR IGNORE INTO library_books (library_id, book_fingerprint, added_to_library_at, file_path)
                    VALUES (?, ?, ?, ?)
                    "#,
                    library_id,
                    fp_str,
                    book_row.added_at,
                    book_row.file_path,
                )
                .execute(&mut *tx)
                .await?;

                let authors = extract_authors(&info.author);
                for (position, author_name) in authors.iter().enumerate() {
                    sqlx::query!(
                        r#"INSERT OR IGNORE INTO authors (name) VALUES (?)"#,
                        author_name
                    )
                    .execute(&mut *tx)
                    .await?;

                    let author_id: i64 = sqlx::query_scalar!(
                        r#"SELECT id FROM authors WHERE name = ?"#,
                        author_name
                    )
                    .fetch_one(&mut *tx)
                    .await?;

                    let pos = position as i64;
                    sqlx::query!(
                        r#"
                        INSERT INTO book_authors (book_fingerprint, author_id, position)
                        VALUES (?, ?, ?)
                        "#,
                        fp_str,
                        author_id,
                        pos
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                for category_name in &info.categories {
                    sqlx::query!(
                        r#"INSERT OR IGNORE INTO categories (name) VALUES (?)"#,
                        category_name
                    )
                    .execute(&mut *tx)
                    .await?;

                    let category_id: i64 = sqlx::query_scalar!(
                        r#"SELECT id FROM categories WHERE name = ?"#,
                        category_name
                    )
                    .fetch_one(&mut *tx)
                    .await?;

                    sqlx::query!(
                        r#"
                        INSERT INTO book_categories (book_fingerprint, category_id)
                        VALUES (?, ?)
                        "#,
                        fp_str,
                        category_id
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                if let Some(reader_info) = &info.reader_info {
                    let rs_row = reader_info_to_reading_state_row(*fp, reader_info);

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
                        "#,
                        rs_row.fingerprint,
                        rs_row.opened,
                        rs_row.current_page,
                        rs_row.pages_count,
                        rs_row.finished,
                        rs_row.dithered,
                        rs_row.zoom_mode,
                        rs_row.scroll_mode,
                        rs_row.page_offset_x,
                        rs_row.page_offset_y,
                        rs_row.rotation,
                        rs_row.cropping_margins_json,
                        rs_row.margin_width,
                        rs_row.screen_margin_width,
                        rs_row.font_family,
                        rs_row.font_size,
                        rs_row.text_align,
                        rs_row.line_height,
                        rs_row.contrast_exponent,
                        rs_row.contrast_gray,
                        rs_row.page_names_json,
                        rs_row.bookmarks_json,
                        rs_row.annotations_json,
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                if let Some(ref toc) = info.toc {
                    sqlx::query!("DELETE FROM toc_entries WHERE book_fingerprint = ?", fp_str)
                        .execute(&mut *tx)
                        .await?;
                    Self::insert_toc_entries(&mut tx, &fp_str, toc, None).await?;
                }
            }

            tx.commit().await?;

            tracing::debug!(count = books.len(), "batch insert complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, books), fields(library_id, count = books.len())))]
    pub fn batch_update_books(&self, library_id: i64, books: &[(Fp, &Info)]) -> Result<(), Error> {
        if books.is_empty() {
            return Ok(());
        }

        tracing::debug!(library_id, count = books.len(), "batch updating books");

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            for (fp, info) in books {
                let fp_str = fp.to_string();

                let book_row = info_to_book_row(*fp, info);

                sqlx::query!(
                    r#"
                    UPDATE books SET
                        title = ?, subtitle = ?, year = ?, language = ?, publisher = ?,
                        series = ?, edition = ?, volume = ?, number = ?, identifier = ?,
                        absolute_path = ?, file_kind = ?, file_size = ?, added_at = ?
                    WHERE fingerprint = ?
                    "#,
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
                    book_row.absolute_path,
                    book_row.file_kind,
                    book_row.file_size,
                    book_row.added_at,
                    fp_str,
                )
                .execute(&mut *tx)
                .await?;

                sqlx::query!(
                    r#"
                    UPDATE library_books SET file_path = ?
                    WHERE library_id = ? AND book_fingerprint = ?
                    "#,
                    book_row.file_path,
                    library_id,
                    fp_str,
                )
                .execute(&mut *tx)
                .await?;

                sqlx::query!(
                    r#"DELETE FROM book_authors WHERE book_fingerprint = ?"#,
                    fp_str
                )
                .execute(&mut *tx)
                .await?;

                let authors = extract_authors(&info.author);
                for (position, author_name) in authors.iter().enumerate() {
                    sqlx::query!(
                        r#"INSERT OR IGNORE INTO authors (name) VALUES (?)"#,
                        author_name
                    )
                    .execute(&mut *tx)
                    .await?;

                    let author_id: i64 = sqlx::query_scalar!(
                        r#"SELECT id FROM authors WHERE name = ?"#,
                        author_name
                    )
                    .fetch_one(&mut *tx)
                    .await?;

                    let pos = position as i64;
                    sqlx::query!(
                        r#"
                        INSERT INTO book_authors (book_fingerprint, author_id, position)
                        VALUES (?, ?, ?)
                        "#,
                        fp_str,
                        author_id,
                        pos
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                sqlx::query!(
                    r#"DELETE FROM book_categories WHERE book_fingerprint = ?"#,
                    fp_str
                )
                .execute(&mut *tx)
                .await?;

                for category_name in &info.categories {
                    sqlx::query!(
                        r#"INSERT OR IGNORE INTO categories (name) VALUES (?)"#,
                        category_name
                    )
                    .execute(&mut *tx)
                    .await?;

                    let category_id: i64 = sqlx::query_scalar!(
                        r#"SELECT id FROM categories WHERE name = ?"#,
                        category_name
                    )
                    .fetch_one(&mut *tx)
                    .await?;

                    sqlx::query!(
                        r#"
                        INSERT INTO book_categories (book_fingerprint, category_id)
                        VALUES (?, ?)
                        "#,
                        fp_str,
                        category_id
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                if let Some(ref toc) = info.toc {
                    sqlx::query!("DELETE FROM toc_entries WHERE book_fingerprint = ?", fp_str)
                        .execute(&mut *tx)
                        .await?;
                    Self::insert_toc_entries(&mut tx, &fp_str, toc, None).await?;
                } else {
                    sqlx::query!("DELETE FROM toc_entries WHERE book_fingerprint = ?", fp_str)
                        .execute(&mut *tx)
                        .await?;
                }
            }

            tx.commit().await?;

            tracing::debug!(count = books.len(), "batch update complete");
            Ok(())
        })
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fps), fields(library_id, count = fps.len())))]
    pub fn batch_delete_books(&self, library_id: i64, fps: &[Fp]) -> Result<(), Error> {
        if fps.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            library_id,
            count = fps.len(),
            "batch deleting books from library"
        );

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            for fp in fps {
                let fp_str = fp.to_string();

                sqlx::query!(
                    r#"DELETE FROM library_books WHERE library_id = ? AND book_fingerprint = ?"#,
                    library_id,
                    fp_str
                )
                .execute(&mut *tx)
                .await?;

                let ref_count: i64 = sqlx::query_scalar!(
                    r#"SELECT COUNT(*) FROM library_books WHERE book_fingerprint = ?"#,
                    fp_str
                )
                .fetch_one(&mut *tx)
                .await?;

                if ref_count == 0 {
                    sqlx::query!(
                        r#"DELETE FROM books WHERE fingerprint = ?"#,
                        fp_str
                    )
                    .execute(&mut *tx)
                    .await?;
                    tracing::debug!(fp = %fp, "book removed from database (no more library references)");
                } else {
                    tracing::debug!(fp = %fp, ref_count, "book kept in database (still referenced by other libraries)");
                }
            }

            tx.commit().await?;

            tracing::debug!(count = fps.len(), "batch delete complete");
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::metadata::ReaderInfo;
    use chrono::Local;
    use std::path::PathBuf;
    use std::str::FromStr;

    fn create_test_db() -> (Database, Db) {
        let db = Database::new(":memory:").expect("failed to create in-memory database");
        db.migrate().expect("failed to run migrations");
        let libdb = Db::new(&db);
        (db, libdb)
    }

    #[test]
    fn test_insert_and_get_book() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_str("0000000000000001").unwrap();

        let info = Info {
            title: "Test Book".to_string(),
            subtitle: "A Test".to_string(),
            author: "John Doe, Jane Smith".to_string(),
            year: "2024".to_string(),
            language: "en".to_string(),
            publisher: "Test Press".to_string(),
            series: "Test Series".to_string(),
            number: "1".to_string(),
            categories: vec!["Fiction".to_string(), "Science".to_string()]
                .into_iter()
                .collect(),
            file: FileInfo {
                path: PathBuf::from("/tmp/test.pdf"),
                kind: "pdf".to_string(),
                size: 1024,
                ..Default::default()
            },
            added: Local::now().naive_local(),
            ..Default::default()
        };

        let library_id = libdb
            .register_library("/tmp/test_library", "Test Library")
            .expect("failed to register library");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved_info = books.iter().find(|(f, _)| *f == fp).map(|(_, i)| i.clone());
        assert!(retrieved_info.is_some(), "book should exist in database");

        let retrieved_info = retrieved_info.unwrap();
        assert_eq!(retrieved_info.title, "Test Book");
        assert_eq!(retrieved_info.subtitle, "A Test");
        assert_eq!(retrieved_info.author, "John Doe, Jane Smith");
        assert_eq!(retrieved_info.year, "2024");
        assert_eq!(retrieved_info.language, "en");
        assert_eq!(retrieved_info.publisher, "Test Press");
        assert_eq!(retrieved_info.series, "Test Series");
        assert_eq!(retrieved_info.number, "1");
        assert_eq!(retrieved_info.file.path, PathBuf::from("/tmp/test.pdf"));
        assert_eq!(retrieved_info.file.kind, "pdf");
        assert_eq!(retrieved_info.file.size, 1024);
    }

    #[test]
    fn test_insert_book_with_reading_state() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_str("0000000000000002").unwrap();

        let reader_info = ReaderInfo {
            current_page: 42,
            pages_count: 100,
            ..Default::default()
        };
        let info = Info {
            title: "Book with Reading State".to_string(),
            author: "Test Author".to_string(),
            file: FileInfo {
                path: PathBuf::from("/tmp/test2.pdf"),
                kind: "pdf".to_string(),
                size: 2048,
                ..Default::default()
            },
            reader_info: Some(reader_info.clone()),
            ..Default::default()
        };

        let library_id = libdb
            .register_library("/tmp/test_library2", "Test Library 2")
            .expect("failed to register library");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved = books
            .iter()
            .find(|(f, _)| *f == fp)
            .map(|(_, i)| i.clone())
            .unwrap();
        assert_eq!(retrieved.title, "Book with Reading State");

        assert!(
            retrieved.reader_info.is_some(),
            "reading state should exist"
        );
        let retrieved_reader = retrieved.reader_info.unwrap();
        assert_eq!(retrieved_reader.current_page, 42);
        assert_eq!(retrieved_reader.pages_count, 100);
        assert!(!retrieved_reader.finished);
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_str("0000000000000003").unwrap();

        let info = Info {
            title: "Book to Delete".to_string(),
            author: "Delete Author".to_string(),
            file: FileInfo {
                path: PathBuf::from("/tmp/delete.pdf"),
                kind: "pdf".to_string(),
                size: 512,
                ..Default::default()
            },
            ..Default::default()
        };

        let library_id = libdb
            .register_library("/tmp/test_library3", "Test Library 3")
            .expect("failed to register library");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        assert!(
            books.iter().any(|(f, _)| *f == fp),
            "book should exist before delete"
        );

        libdb
            .delete_book(library_id, fp)
            .expect("failed to delete book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        assert!(
            !books.iter().any(|(f, _)| *f == fp),
            "book should not exist after delete"
        );
    }

    #[test]
    fn test_multiple_books() {
        let (_db, libdb) = create_test_db();
        let library_id = libdb
            .register_library("/tmp/test_library4", "Test Library 4")
            .expect("failed to register library");

        for i in 1..=5 {
            let fp = Fp::from_str(&format!("{:016X}", i)).unwrap();
            let info = Info {
                title: format!("Book {}", i),
                author: format!("Author {}", i),
                file: FileInfo {
                    path: PathBuf::from(format!("/tmp/book{}.pdf", i)),
                    kind: "pdf".to_string(),
                    size: (i * 100) as u64,
                    ..Default::default()
                },
                ..Default::default()
            };

            libdb
                .insert_book(library_id, fp, &info)
                .expect("failed to insert book");
        }

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        for i in 1..=5 {
            let fp = Fp::from_str(&format!("{:016X}", i)).unwrap();
            let retrieved = books
                .iter()
                .find(|(f, _)| *f == fp)
                .map(|(_, i)| i.clone())
                .unwrap();
            assert_eq!(retrieved.title, format!("Book {}", i));
            assert_eq!(retrieved.author, format!("Author {}", i));
        }
    }

    #[test]
    fn test_update_book() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_str("0000000000000004").unwrap();

        let mut info = Info {
            title: "Original Title".to_string(),
            author: "Original Author".to_string(),
            file: FileInfo {
                path: PathBuf::from("/tmp/update.pdf"),
                kind: "pdf".to_string(),
                size: 1024,
                ..Default::default()
            },
            ..Default::default()
        };

        let library_id = libdb
            .register_library("/tmp/test_library5", "Test Library 5")
            .expect("failed to register library");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        info.title = "Updated Title".to_string();
        info.author = "Updated Author".to_string();
        info.year = "2025".to_string();

        libdb
            .update_book(library_id, fp, &info)
            .expect("failed to update book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let updated = books
            .iter()
            .find(|(f, _)| *f == fp)
            .map(|(_, i)| i.clone())
            .unwrap();
        assert_eq!(updated.title, "Updated Title");
        assert_eq!(updated.author, "Updated Author");
        assert_eq!(updated.year, "2025");
    }

    #[test]
    fn test_get_all_books() {
        let (_db, libdb) = create_test_db();
        let library_id = libdb
            .register_library("/tmp/test_library6", "Test Library 6")
            .expect("failed to register library");

        for i in 1..=3 {
            let fp = Fp::from_str(&format!("{:016X}", i)).unwrap();
            let info = Info {
                title: format!("Book {}", i),
                author: format!("Author {}", i),
                file: FileInfo {
                    path: PathBuf::from(format!("/tmp/book{}.pdf", i)),
                    kind: "pdf".to_string(),
                    size: (i * 100) as u64,
                    ..Default::default()
                },
                ..Default::default()
            };

            libdb
                .insert_book(library_id, fp, &info)
                .expect("failed to insert book");
        }

        let all_books = libdb
            .get_all_books(library_id)
            .expect("failed to get all books");
        assert_eq!(all_books.len(), 3);

        let titles: Vec<String> = all_books
            .iter()
            .map(|(_, info)| info.title.clone())
            .collect();
        assert!(titles.contains(&"Book 1".to_string()));
        assert!(titles.contains(&"Book 2".to_string()));
        assert!(titles.contains(&"Book 3".to_string()));
    }

    #[test]
    fn test_reading_state_crud() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_str("0000000000000005").unwrap();

        let info = Info {
            title: "Book with State".to_string(),
            author: "State Author".to_string(),
            file: FileInfo {
                path: PathBuf::from("/tmp/state.pdf"),
                kind: "pdf".to_string(),
                size: 1024,
                ..Default::default()
            },
            ..Default::default()
        };

        let library_id = libdb
            .register_library("/tmp/test_library7", "Test Library 7")
            .expect("failed to register library");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let mut reader_info = ReaderInfo {
            current_page: 50,
            pages_count: 200,
            ..Default::default()
        };

        libdb
            .save_reading_state(fp, &reader_info)
            .expect("failed to save reading state");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved = books
            .iter()
            .find(|(f, _)| *f == fp)
            .map(|(_, i)| i.clone())
            .unwrap();
        let retrieved_reader = retrieved.reader_info.unwrap();

        assert_eq!(retrieved_reader.current_page, 50);
        assert_eq!(retrieved_reader.pages_count, 200);
        assert!(!retrieved_reader.finished);
        reader_info.current_page = 100;
        reader_info.finished = true;

        libdb
            .save_reading_state(fp, &reader_info)
            .expect("failed to update reading state");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let updated = books
            .iter()
            .find(|(f, _)| *f == fp)
            .map(|(_, i)| i.clone())
            .unwrap();
        let updated_reader = updated.reader_info.unwrap();

        assert_eq!(updated_reader.current_page, 100);
        assert!(updated_reader.finished);
    }

    #[test]
    fn test_batch_insert_books() {
        let (_db, libdb) = create_test_db();
        let library_id = libdb
            .register_library("/tmp/test_library8", "Test Library 8")
            .expect("failed to register library");

        let mut books = Vec::new();
        for i in 1..=5 {
            let fp = Fp::from_str(&format!("{:016X}", i + 100)).unwrap();
            let info = Info {
                title: format!("Batch Book {}", i),
                author: format!("Batch Author {}, Co-Author {}", i, i + 1),
                year: format!("{}", 2020 + i),
                file: FileInfo {
                    path: PathBuf::from(format!("/tmp/batch{}.pdf", i)),
                    kind: "pdf".to_string(),
                    size: (i * 100) as u64,
                    ..Default::default()
                },
                ..Default::default()
            };
            books.push((fp, info));
        }

        let book_refs: Vec<(Fp, &Info)> = books.iter().map(|(fp, info)| (*fp, info)).collect();

        libdb
            .batch_insert_books(library_id, &book_refs)
            .expect("failed to batch insert books");

        let all_books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        for (fp, info) in &books {
            let retrieved = all_books
                .iter()
                .find(|(f, _)| *f == *fp)
                .map(|(_, i)| i.clone())
                .expect("book should exist");
            assert_eq!(retrieved.title, info.title);
            assert_eq!(retrieved.author, info.author);
            assert_eq!(retrieved.year, info.year);
        }

        let all_books = libdb
            .get_all_books(library_id)
            .expect("failed to get all books");
        assert_eq!(all_books.len(), 5);
    }

    #[test]
    fn test_batch_update_books() {
        let (_db, libdb) = create_test_db();
        let library_id = libdb
            .register_library("/tmp/test_library9", "Test Library 9")
            .expect("failed to register library");

        let mut books = Vec::new();
        for i in 1..=3 {
            let fp = Fp::from_str(&format!("{:016X}", i + 200)).unwrap();
            let mut info = Info {
                title: format!("Original Book {}", i),
                author: format!("Original Author {}", i),
                file: FileInfo {
                    path: PathBuf::from(format!("/tmp/update{}.pdf", i)),
                    kind: "pdf".to_string(),
                    size: (i * 100) as u64,
                    ..Default::default()
                },
                ..Default::default()
            };
            libdb
                .insert_book(library_id, fp, &info)
                .expect("failed to insert book");

            info.title = format!("Updated Book {}", i);
            info.author = format!("Updated Author {}", i);
            info.year = format!("{}", 2024 + i);
            books.push((fp, info));
        }

        let book_refs: Vec<(Fp, &Info)> = books.iter().map(|(fp, info)| (*fp, info)).collect();

        libdb
            .batch_update_books(library_id, &book_refs)
            .expect("failed to batch update books");

        let all_books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        for (fp, info) in &books {
            let retrieved = all_books
                .iter()
                .find(|(f, _)| *f == *fp)
                .map(|(_, i)| i.clone())
                .expect("book should exist");
            assert_eq!(retrieved.title, info.title);
            assert_eq!(retrieved.author, info.author);
            assert_eq!(retrieved.year, info.year);
        }
    }

    #[test]
    fn test_batch_delete_books() {
        let (_db, libdb) = create_test_db();
        let library_id = libdb
            .register_library("/tmp/test_library10", "Test Library 10")
            .expect("failed to register library");

        let mut fps = Vec::new();
        for i in 1..=4 {
            let fp = Fp::from_str(&format!("{:016X}", i + 300)).unwrap();
            let info = Info {
                title: format!("Delete Book {}", i),
                author: format!("Delete Author {}", i),
                file: FileInfo {
                    path: PathBuf::from(format!("/tmp/delete{}.pdf", i)),
                    kind: "pdf".to_string(),
                    size: (i * 100) as u64,
                    ..Default::default()
                },
                ..Default::default()
            };
            libdb
                .insert_book(library_id, fp, &info)
                .expect("failed to insert book");
            fps.push(fp);
        }

        let all_before = libdb
            .get_all_books(library_id)
            .expect("failed to get all books");
        assert_eq!(all_before.len(), 4);

        libdb
            .batch_delete_books(library_id, &fps)
            .expect("failed to batch delete books");

        let all_after = libdb
            .get_all_books(library_id)
            .expect("failed to get all books");
        assert_eq!(all_after.len(), 0);
    }

    #[test]
    fn test_batch_operations_with_empty_input() {
        let (_db, libdb) = create_test_db();
        let library_id = libdb
            .register_library("/tmp/test_library11", "Test Library 11")
            .expect("failed to register library");

        let empty_books: Vec<(Fp, &Info)> = Vec::new();
        let empty_fps: Vec<Fp> = Vec::new();

        libdb
            .batch_insert_books(library_id, &empty_books)
            .expect("empty batch insert should succeed");
        libdb
            .batch_update_books(library_id, &empty_books)
            .expect("empty batch update should succeed");
        libdb
            .batch_delete_books(library_id, &empty_fps)
            .expect("empty batch delete should succeed");
    }

    #[test]
    fn test_categories_round_trip() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_str("0000000000000099").unwrap();

        let info = Info {
            title: "Categorized Book".to_string(),
            author: "Cat Author".to_string(),
            file: FileInfo {
                path: PathBuf::from("/tmp/cat.pdf"),
                kind: "pdf".to_string(),
                size: 512,
                ..Default::default()
            },
            categories: ["Fiction", "Science", "History"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..Default::default()
        };

        let library_id = libdb
            .register_library("/tmp/test_library_cat", "Cat Library")
            .expect("failed to register library");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved = books
            .iter()
            .find(|(f, _)| *f == fp)
            .map(|(_, i)| i.clone())
            .expect("book should exist");

        assert_eq!(retrieved.categories, info.categories);
    }

    #[test]
    fn test_categories_updated_on_update_book() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_str("000000000000009A").unwrap();

        let mut info = Info {
            title: "Updateable Book".to_string(),
            author: "Update Author".to_string(),
            file: FileInfo {
                path: PathBuf::from("/tmp/upd_cat.pdf"),
                kind: "pdf".to_string(),
                size: 512,
                ..Default::default()
            },
            categories: ["OldCat"].iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };

        let library_id = libdb
            .register_library("/tmp/test_library_upd_cat", "Upd Cat Library")
            .expect("failed to register library");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        info.categories = ["NewCat1", "NewCat2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        libdb
            .update_book(library_id, fp, &info)
            .expect("failed to update book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved = books
            .iter()
            .find(|(f, _)| *f == fp)
            .map(|(_, i)| i.clone())
            .expect("book should exist");

        assert_eq!(retrieved.categories, info.categories);
    }

    #[test]
    fn test_batch_insert_with_reading_state() {
        let (_db, libdb) = create_test_db();
        let library_id = libdb
            .register_library("/tmp/test_library12", "Test Library 12")
            .expect("failed to register library");

        let mut books = Vec::new();
        for i in 1..=3 {
            let fp = Fp::from_str(&format!("{:016X}", i + 400)).unwrap();
            let reader_info = ReaderInfo {
                current_page: i * 10,
                pages_count: i * 100,
                finished: i % 2 == 0,
                ..Default::default()
            };
            let info = Info {
                title: format!("Book with State {}", i),
                author: format!("State Author {}", i),
                file: FileInfo {
                    path: PathBuf::from(format!("/tmp/state{}.pdf", i)),
                    kind: "pdf".to_string(),
                    size: (i * 100) as u64,
                    ..Default::default()
                },
                reader_info: Some(reader_info),
                ..Default::default()
            };

            books.push((fp, info));
        }

        let book_refs: Vec<(Fp, &Info)> = books.iter().map(|(fp, info)| (*fp, info)).collect();

        libdb
            .batch_insert_books(library_id, &book_refs)
            .expect("failed to batch insert books with reading state");

        let all_books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        for (fp, info) in &books {
            let retrieved = all_books
                .iter()
                .find(|(f, _)| *f == *fp)
                .map(|(_, i)| i.clone())
                .expect("book should exist");
            assert_eq!(retrieved.title, info.title);

            assert!(
                retrieved.reader_info.is_some(),
                "reading state should exist"
            );
            let retrieved_state = retrieved.reader_info.unwrap();
            let original_state = info.reader_info.as_ref().unwrap();
            assert_eq!(retrieved_state.current_page, original_state.current_page);
            assert_eq!(retrieved_state.pages_count, original_state.pages_count);
            assert_eq!(retrieved_state.finished, original_state.finished);
        }
    }
}
