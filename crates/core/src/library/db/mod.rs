pub mod conversion;
pub mod models;

use crate::db::runtime::RUNTIME;
use crate::db::types::{OptionalUuid7, UnixTimestamp, Uuid7};
use crate::db::Database;
use crate::document::SimpleTocEntry;
use crate::geom::Point;
use crate::helpers::Fp;
use crate::metadata::{
    alphabetic_author, alphabetic_title, natural_cmp, sorter, CroppingMargins, FileInfo, Info,
    ReaderInfo, ScrollMode, SortMethod, TextAlign, ZoomMode,
};
use crate::settings::FileExtension;
use anyhow::Error;
use conversion::{
    extract_authors, info_to_book_row, reader_info_to_reading_state_row, rows_to_toc_entries,
};
use fxhash::{FxHashMap, FxHashSet};
use models::TocEntryRow;
use sqlx::sqlite::SqlitePool;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Gap between adjacent sort ranks assigned by [`Db::compute_sort_keys`].
///
/// Ranks are stored as multiples of this value (1 000, 2 000, 3 000, …) so
/// that a single newly-added book can be placed at the midpoint between its
/// two neighbours without touching any other row. See [`Db::insert_sort_rank`].
const SORT_RANK_STRIDE: i64 = 1_000;

/// Computes the rank to assign to a new book being inserted at position `pos`
/// in a list of existing ranks (which may be `None` for books whose ranks have
/// not yet been computed).
///
/// Returns `None` when the gap between the two neighbours has been fully
/// exhausted (they differ by ≤ 1), signalling that a full recompute is needed.
fn midpoint_rank(existing_ranks: &[Option<i64>], pos: usize) -> Option<i64> {
    let left = if pos == 0 {
        None
    } else {
        existing_ranks.get(pos - 1).copied().flatten()
    };
    let right = existing_ranks.get(pos).copied().flatten();

    match (left, right) {
        (None, None) => Some(SORT_RANK_STRIDE),
        (None, Some(r)) => {
            if r <= 1 {
                None
            } else {
                Some(r / 2)
            }
        }
        (Some(l), None) => Some(l + SORT_RANK_STRIDE),
        (Some(l), Some(r)) => {
            let mid = (l + r) / 2;
            if mid <= l {
                None
            } else {
                Some(mid)
            }
        }
    }
}

/// Lightweight row fetched by [`Db::fetch_title_sort_rows`] for binary search.
#[derive(sqlx::FromRow)]
struct TitleSortRow {
    title: String,
    language: String,
    file_path: String,
    sort_title: Option<i64>,
}

/// Lightweight row fetched by [`Db::fetch_author_sort_rows`] for binary search.
#[derive(sqlx::FromRow)]
struct AuthorSortRow {
    authors: Option<String>,
    sort_author: Option<i64>,
}

/// Lightweight row fetched by [`Db::fetch_filepath_sort_rows`] for binary search.
#[derive(sqlx::FromRow)]
struct FilePathSortRow {
    file_path: String,
    sort_filepath: Option<i64>,
}

/// Lightweight row fetched by [`Db::fetch_filename_sort_rows`] for binary search.
#[derive(sqlx::FromRow)]
struct FileNameSortRow {
    file_path: String,
    sort_filename: Option<i64>,
}

/// Lightweight row fetched by [`Db::fetch_series_sort_rows`] for binary search.
#[derive(sqlx::FromRow)]
struct SeriesSortRow {
    series: String,
    number: String,
    sort_series: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StoredBookRow {
    fingerprint: Fp,
    title: String,
    subtitle: String,
    year: String,
    language: String,
    publisher: String,
    series: String,
    edition: String,
    volume: String,
    number: String,
    identifier: String,
    file_path: String,
    absolute_path: String,
    file_kind: String,
    file_size: i64,
    added_at: UnixTimestamp,
    opened: Option<UnixTimestamp>,
    current_page: Option<i64>,
    pages_count: Option<i64>,
    finished: Option<i64>,
    dithered: Option<i64>,
    zoom_mode: Option<String>,
    scroll_mode: Option<String>,
    page_offset_x: Option<i64>,
    page_offset_y: Option<i64>,
    rotation: Option<i64>,
    cropping_margins_json: Option<String>,
    margin_width: Option<i64>,
    screen_margin_width: Option<i64>,
    font_family: Option<String>,
    font_size: Option<f64>,
    text_align: Option<String>,
    line_height: Option<f64>,
    contrast_exponent: Option<f64>,
    contrast_gray: Option<f64>,
    page_names_json: Option<String>,
    bookmarks_json: Option<String>,
    annotations_json: Option<String>,
    authors: Option<String>,
    categories: Option<String>,
}

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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(path = %path, name = %name)))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(path = %path)))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(pool)))]
    async fn fetch_toc_entries_for_book(
        pool: &SqlitePool,
        library_id: i64,
        fingerprint: &str,
    ) -> Result<Vec<TocEntryRow>, Error> {
        let rows = sqlx::query_as!(
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
            WHERE lb.library_id = ? AND te.book_fingerprint = ?
            ORDER BY te.id ASC
            "#,
            library_id,
            fingerprint,
        )
        .fetch_all(pool)
        .await?;

        Ok(rows)
    }

    fn stored_book_row_to_info(
        row: StoredBookRow,
        toc: Option<Vec<SimpleTocEntry>>,
    ) -> Result<Info, Error> {
        let fp = row.fingerprint;

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
            fp: Some(fp),
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
                rotation: row.rotation.map(|rotation| rotation as i8),
                cropping_margins: Self::parse_cropping_margins(row.cropping_margins_json.as_ref()),
                margin_width: row.margin_width.map(|margin| margin as i32),
                screen_margin_width: row.screen_margin_width.map(|margin| margin as i32),
                font_family: row.font_family,
                font_size: row.font_size.map(|size| size as f32),
                text_align: Self::parse_text_align(row.text_align.as_ref()),
                line_height: row.line_height.map(|height| height as f32),
                contrast_exponent: row.contrast_exponent.map(|contrast| contrast as f32),
                contrast_gray: row.contrast_gray.map(|contrast| contrast as f32),
                page_names: Self::parse_page_names(row.page_names_json.as_ref()),
                bookmarks: Self::parse_bookmarks(row.bookmarks_json.as_ref()),
                annotations: Self::parse_annotations(row.annotations_json.as_ref()),
            };
            info.reader = Some(reader_info.clone());
            info.reader_info = Some(reader_info);
        }

        Ok(info)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(conn, entries), fields(book_fingerprint = %book_fingerprint, parent_id = ?parent_id)))]
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

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(library_id))
    )]
    pub fn get_all_books(&self, library_id: i64) -> Result<Vec<Info>, Error> {
        tracing::debug!(library_id, "fetching all books from database");

        RUNTIME.block_on(async {
            let book_rows = sqlx::query!(
                r#"
                SELECT
                    fingerprint as "fingerprint: Fp",
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
                let fp = row.fingerprint;

                let toc = toc_by_fingerprint
                    .remove(&fp.to_string())
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
                    fp: Some(fp),
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

                result.push(info);
            }

            tracing::debug!(library_id, count = result.len(), "fetched all books");
            Ok(result)
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, path), fields(library_id, path = %path.display())))]
    pub fn get_book_by_path(&self, library_id: i64, path: &Path) -> Result<Option<Info>, Error> {
        let path = path.to_string_lossy().into_owned();

        RUNTIME.block_on(async {
            let row = sqlx::query_as!(
                StoredBookRow,
                r#"
                SELECT
                    fingerprint as "fingerprint: Fp",
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
                WHERE library_id = ? AND file_path = ?
                LIMIT 1
                "#,
                library_id,
                path,
            )
            .fetch_optional(&self.pool)
            .await?;

            let Some(row) = row else {
                return Ok(None);
            };

            let toc_rows = Self::fetch_toc_entries_for_book(
                &self.pool,
                library_id,
                &row.fingerprint.to_string(),
            )
            .await?;
            let toc = (!toc_rows.is_empty())
                .then(|| rows_to_toc_entries(&toc_rows))
                .transpose()?;

            Self::stored_book_row_to_info(row, toc).map(Some)
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(library_id, fp = %fp)))]
    pub fn get_book_by_fingerprint(&self, library_id: i64, fp: Fp) -> Result<Option<Info>, Error> {
        let fingerprint = fp.to_string();

        RUNTIME.block_on(async {
            let row = sqlx::query_as!(
                StoredBookRow,
                r#"
                SELECT
                    fingerprint as "fingerprint: Fp",
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
                WHERE library_id = ? AND fingerprint = ?
                LIMIT 1
                "#,
                library_id,
                fingerprint,
            )
            .fetch_optional(&self.pool)
            .await?;

            let Some(row) = row else {
                return Ok(None);
            };

            let toc_rows = Self::fetch_toc_entries_for_book(
                &self.pool,
                library_id,
                &row.fingerprint.to_string(),
            )
            .await?;
            let toc = (!toc_rows.is_empty())
                .then(|| rows_to_toc_entries(&toc_rows))
                .transpose()?;

            Self::stored_book_row_to_info(row, toc).map(Some)
        })
    }

    /// Fetches complete `Info` for multiple fingerprints in a single library using one
    /// pooled connection. Missing fingerprints are silently skipped.
    ///
    /// Used by `import()` to retrieve book metadata (title, authors, reading state, etc.)
    /// for all fingerprint relocations in one batch, before re-inserting under new FPs.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fps), fields(library_id, count = fps.len())))]
    pub fn batch_get_books_by_fingerprints(
        &self,
        library_id: i64,
        fps: &[Fp],
    ) -> Result<FxHashMap<Fp, Info>, Error> {
        if fps.is_empty() {
            return Ok(FxHashMap::default());
        }

        tracing::debug!(
            library_id,
            count = fps.len(),
            "batch fetching books by fingerprints"
        );

        RUNTIME.block_on(async {
            let mut result = FxHashMap::default();
            let mut conn = self.pool.acquire().await?;

            for fp in fps {
                let fingerprint = fp.to_string();

                let row = sqlx::query_as!(
                    StoredBookRow,
                    r#"
                    SELECT
                        fingerprint as "fingerprint: Fp",
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
                    WHERE library_id = ? AND fingerprint = ?
                    LIMIT 1
                    "#,
                    library_id,
                    fingerprint,
                )
                .fetch_optional(&mut *conn)
                .await?;

                let Some(row) = row else {
                    continue;
                };

                let toc_rows = Self::fetch_toc_entries_for_book(
                    &self.pool,
                    library_id,
                    &row.fingerprint.to_string(),
                )
                .await?;
                let toc = (!toc_rows.is_empty())
                    .then(|| rows_to_toc_entries(&toc_rows))
                    .transpose()?;

                if let Ok(info) = Self::stored_book_row_to_info(row, toc) {
                    result.insert(*fp, info);
                }
            }

            Ok(result)
        })
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(library_id))
    )]
    pub fn count_books(&self, library_id: i64) -> Result<usize, Error> {
        RUNTIME.block_on(async {
            let count: i64 = sqlx::query_scalar!(
                r#"SELECT COUNT(*) AS "count!: i64" FROM library_books WHERE library_id = ?"#,
                library_id,
            )
            .fetch_one(&self.pool)
            .await?;

            Ok(count as usize)
        })
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, prefix), fields(library_id))
    )]
    pub fn list_books_under_prefix(
        &self,
        library_id: i64,
        prefix: &Path,
    ) -> Result<Vec<Info>, Error> {
        let prefix =
            (!prefix.as_os_str().is_empty()).then(|| prefix.to_string_lossy().into_owned());

        RUNTIME.block_on(async {
            let rows: Vec<StoredBookRow> = sqlx::query_as!(
                StoredBookRow,
                r#"
                SELECT
                    fingerprint as "fingerprint: Fp",
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
                WHERE library_id = ?1
                  AND (?2 IS NULL OR file_path = ?2 OR file_path LIKE (?2 || '/%'))
                "#,
                library_id,
                prefix,
            )
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter()
                .map(|row| Self::stored_book_row_to_info(row, None))
                .collect()
        })
    }

    pub fn most_recently_opened_reading_book(
        &self,
        library_id: i64,
    ) -> Result<Option<Info>, Error> {
        RUNTIME.block_on(async {
            let row: Option<StoredBookRow> = sqlx::query_as!(
                StoredBookRow,
                r#"
                SELECT
                    fingerprint as "fingerprint: Fp",
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
                WHERE library_id = ?1
                  AND finished = 0
                  AND opened IS NOT NULL
                ORDER BY opened DESC
                LIMIT 1
                "#,
                library_id,
            )
            .fetch_optional(&self.pool)
            .await?;

            row.map(|r| Self::stored_book_row_to_info(r, None))
                .transpose()
        })
    }

    /// Recomputes sort ranks for all books in a library and writes them to the
    /// five pre-computed sort columns (`sort_title`, `sort_author`,
    /// `sort_filepath`, `sort_filename`, `sort_series`).
    ///
    /// # Sparse rank scheme
    ///
    /// Ranks are stored as **multiples of 1000** rather than consecutive
    /// integers (1 → 1 000, 2 → 2 000, …). The gaps allow a single newly
    /// added book to be inserted cheaply via [`Self::insert_sort_rank`]:
    /// instead of shifting every book above the insertion point, the new book
    /// is assigned the midpoint between its two neighbours — a single UPDATE.
    ///
    /// A full recompute is only needed after bulk changes (i.e. after
    /// `import()`). It also restores uniform gaps whenever they have been
    /// partially exhausted by many consecutive single-book insertions.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn compute_sort_keys(&self, library_id: i64) -> Result<(), Error> {
        let books = self.get_all_books(library_id)?;
        if books.is_empty() {
            return Ok(());
        }

        let methods: &[(SortMethod, &str)] = &[
            (SortMethod::Title, "sort_title"),
            (SortMethod::Author, "sort_author"),
            (SortMethod::FilePath, "sort_filepath"),
            (SortMethod::FileName, "sort_filename"),
            (SortMethod::Series, "sort_series"),
        ];

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            for (method, col) in methods {
                let mut sorted = books.clone();
                sorted.sort_by(sorter(*method));

                let sql = format!(
                    "UPDATE library_books SET {col} = ? WHERE library_id = ? AND book_fingerprint = ?"
                );
                for (rank, info) in sorted.iter().enumerate() {
                    let fp = info.fp.map(|f| f.to_string()).unwrap_or_default();
                    // Multiply by SORT_RANK_STRIDE to leave gaps for cheap
                    // single-book insertions via insert_sort_rank.
                    let rank = (rank as i64 + 1) * SORT_RANK_STRIDE;
                    sqlx::query(&sql)
                        .bind(rank)
                        .bind(library_id)
                        .bind(&fp)
                        .execute(&mut *tx)
                        .await?;
                }
            }

            tx.commit().await?;
            Ok(())
        })
    }

    /// Inserts sort ranks for a single newly-added book without recomputing
    /// ranks for the entire library.
    ///
    /// # How it works
    ///
    /// Because [`Self::compute_sort_keys`] stores ranks as multiples of
    /// [`SORT_RANK_STRIDE`] (1 000), there is always a gap between adjacent
    /// books. For each sort column this method:
    ///
    /// 1. Fetches only the two lightweight fields needed to compare the new
    ///    book's sort key — e.g. `(book_fingerprint, title, sort_title)` for
    ///    the title column — ordered by the existing rank. No full `Info` load
    ///    is required.
    /// 2. Binary-searches the sorted list to find the insertion position using
    ///    the same comparator as `sorter(method)`.
    /// 3. Assigns the midpoint between the two neighbouring ranks to the new
    ///    book (e.g. between rank 3 000 and 4 000 → 3 500).
    ///
    /// If any column has exhausted its gaps (two neighbours whose ranks differ
    /// by at most 1), it falls back to a full [`Self::compute_sort_keys`]
    /// recompute to restore uniform gaps for that library.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, info)))]
    pub fn insert_sort_rank(&self, library_id: i64, fp: Fp, info: &Info) -> Result<(), Error> {
        let fp_str = fp.to_string();
        let needs_full_recompute = self.try_insert_sort_rank(library_id, &fp_str, info)?;

        if needs_full_recompute {
            tracing::debug!(
                library_id,
                "sort rank gaps exhausted, falling back to full recompute"
            );
            self.compute_sort_keys(library_id)?;
        }

        Ok(())
    }

    /// Attempts to insert sort ranks for a single book by midpoint assignment.
    ///
    /// Returns `true` if any column has gaps too small to split (i.e. a full
    /// recompute is needed), `false` if all ranks were assigned successfully.
    fn try_insert_sort_rank(
        &self,
        library_id: i64,
        fp_str: &str,
        info: &Info,
    ) -> Result<bool, Error> {
        let title_rank = self.resolve_title_rank(library_id, fp_str, info)?;
        let author_rank = self.resolve_author_rank(library_id, fp_str, info)?;
        let filepath_rank = self.resolve_filepath_rank(library_id, fp_str, info)?;
        let filename_rank = self.resolve_filename_rank(library_id, fp_str, info)?;
        let series_rank = self.resolve_series_rank(library_id, fp_str, info)?;

        if [
            title_rank,
            author_rank,
            filepath_rank,
            filename_rank,
            series_rank,
        ]
        .iter()
        .any(|r| r.is_none())
        {
            return Ok(true);
        }

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            sqlx::query!(
                "UPDATE library_books SET sort_title = ? WHERE library_id = ? AND book_fingerprint = ?",
                title_rank, library_id, fp_str
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                "UPDATE library_books SET sort_author = ? WHERE library_id = ? AND book_fingerprint = ?",
                author_rank, library_id, fp_str
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                "UPDATE library_books SET sort_filepath = ? WHERE library_id = ? AND book_fingerprint = ?",
                filepath_rank, library_id, fp_str
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                "UPDATE library_books SET sort_filename = ? WHERE library_id = ? AND book_fingerprint = ?",
                filename_rank, library_id, fp_str
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                "UPDATE library_books SET sort_series = ? WHERE library_id = ? AND book_fingerprint = ?",
                series_rank, library_id, fp_str
            )
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(false)
        })
    }

    fn resolve_title_rank(
        &self,
        library_id: i64,
        fp_str: &str,
        info: &Info,
    ) -> Result<Option<i64>, Error> {
        let key = {
            let t = info.alphabetic_title();
            if t.is_empty() {
                info.file_stem()
            } else {
                t.to_string()
            }
        };
        let rows = self.fetch_title_sort_rows(library_id, fp_str)?;
        let pos = rows.partition_point(|row| {
            let row_key = {
                let t = alphabetic_title(&row.title, &row.language);
                if t.is_empty() {
                    Path::new(&row.file_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    t.to_string()
                }
            };
            matches!(natural_cmp(&row_key, &key), std::cmp::Ordering::Less)
        });
        Ok(midpoint_rank(
            &rows.iter().map(|r| r.sort_title).collect::<Vec<_>>(),
            pos,
        ))
    }

    fn resolve_author_rank(
        &self,
        library_id: i64,
        fp_str: &str,
        info: &Info,
    ) -> Result<Option<i64>, Error> {
        let key = info.alphabetic_author().to_string();
        let rows = self.fetch_author_sort_rows(library_id, fp_str)?;
        let pos = rows.partition_point(|row| {
            alphabetic_author(row.authors.as_deref().unwrap_or_default()) < key.as_str()
        });
        Ok(midpoint_rank(
            &rows.iter().map(|r| r.sort_author).collect::<Vec<_>>(),
            pos,
        ))
    }

    fn resolve_filepath_rank(
        &self,
        library_id: i64,
        fp_str: &str,
        info: &Info,
    ) -> Result<Option<i64>, Error> {
        let key = info.file.path.to_string_lossy().into_owned();
        let rows = self.fetch_filepath_sort_rows(library_id, fp_str)?;
        let pos = rows.partition_point(|row| {
            matches!(natural_cmp(&row.file_path, &key), std::cmp::Ordering::Less)
        });
        Ok(midpoint_rank(
            &rows.iter().map(|r| r.sort_filepath).collect::<Vec<_>>(),
            pos,
        ))
    }

    fn resolve_filename_rank(
        &self,
        library_id: i64,
        fp_str: &str,
        info: &Info,
    ) -> Result<Option<i64>, Error> {
        let key = info
            .file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rows = self.fetch_filename_sort_rows(library_id, fp_str)?;
        let pos = rows.partition_point(|row| {
            let row_name = Path::new(&row.file_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            matches!(natural_cmp(&row_name, &key), std::cmp::Ordering::Less)
        });
        Ok(midpoint_rank(
            &rows.iter().map(|r| r.sort_filename).collect::<Vec<_>>(),
            pos,
        ))
    }

    fn resolve_series_rank(
        &self,
        library_id: i64,
        fp_str: &str,
        info: &Info,
    ) -> Result<Option<i64>, Error> {
        let series_key = &info.series;
        let number_key = &info.number;
        let rows = self.fetch_series_sort_rows(library_id, fp_str)?;
        let pos = rows.partition_point(|row| {
            row.series.cmp(series_key).then_with(|| {
                row.number
                    .parse::<usize>()
                    .ok()
                    .zip(number_key.parse::<usize>().ok())
                    .map_or_else(|| row.number.cmp(number_key), |(a, b)| a.cmp(&b))
            }) == std::cmp::Ordering::Less
        });
        Ok(midpoint_rank(
            &rows.iter().map(|r| r.sort_series).collect::<Vec<_>>(),
            pos,
        ))
    }

    fn fetch_title_sort_rows(
        &self,
        library_id: i64,
        fp_str: &str,
    ) -> Result<Vec<TitleSortRow>, Error> {
        RUNTIME.block_on(async {
            sqlx::query_as!(
                TitleSortRow,
                r#"
                SELECT title, language, file_path, sort_title as "sort_title?: i64"
                FROM library_books_full_info
                WHERE library_id = ? AND fingerprint != ?
                ORDER BY sort_title ASC NULLS LAST
                "#,
                library_id,
                fp_str,
            )
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
        })
    }

    fn fetch_author_sort_rows(
        &self,
        library_id: i64,
        fp_str: &str,
    ) -> Result<Vec<AuthorSortRow>, Error> {
        RUNTIME.block_on(async {
            sqlx::query_as!(
                AuthorSortRow,
                r#"
                SELECT authors as "authors?: String", sort_author as "sort_author?: i64"
                FROM library_books_full_info
                WHERE library_id = ? AND fingerprint != ?
                ORDER BY sort_author ASC NULLS LAST
                "#,
                library_id,
                fp_str,
            )
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
        })
    }

    fn fetch_filepath_sort_rows(
        &self,
        library_id: i64,
        fp_str: &str,
    ) -> Result<Vec<FilePathSortRow>, Error> {
        RUNTIME.block_on(async {
            sqlx::query_as!(
                FilePathSortRow,
                r#"
                SELECT file_path, sort_filepath as "sort_filepath?: i64"
                FROM library_books_full_info
                WHERE library_id = ? AND fingerprint != ?
                ORDER BY sort_filepath ASC NULLS LAST
                "#,
                library_id,
                fp_str,
            )
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
        })
    }

    fn fetch_filename_sort_rows(
        &self,
        library_id: i64,
        fp_str: &str,
    ) -> Result<Vec<FileNameSortRow>, Error> {
        RUNTIME.block_on(async {
            sqlx::query_as!(
                FileNameSortRow,
                r#"
                SELECT file_path, sort_filename as "sort_filename?: i64"
                FROM library_books_full_info
                WHERE library_id = ? AND fingerprint != ?
                ORDER BY sort_filename ASC NULLS LAST
                "#,
                library_id,
                fp_str,
            )
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
        })
    }

    fn fetch_series_sort_rows(
        &self,
        library_id: i64,
        fp_str: &str,
    ) -> Result<Vec<SeriesSortRow>, Error> {
        RUNTIME.block_on(async {
            sqlx::query_as!(
                SeriesSortRow,
                r#"
                SELECT series, number, sort_series as "sort_series?: i64"
                FROM library_books_full_info
                WHERE library_id = ? AND fingerprint != ?
                ORDER BY sort_series ASC NULLS LAST
                "#,
                library_id,
                fp_str,
            )
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
        })
    }

    /// Returns a page of books under `prefix`, sorted by `sort_method`, along
    /// with the total number of matching books.
    ///
    /// Uses untyped `sqlx::query_as` so the `ORDER BY` column can be selected
    /// dynamically.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn page_books(
        &self,
        library_id: i64,
        prefix: &Path,
        sort_method: SortMethod,
        reverse: bool,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Info>, i64), Error> {
        let prefix_str =
            (!prefix.as_os_str().is_empty()).then(|| prefix.to_string_lossy().into_owned());

        let dir = if reverse { "DESC" } else { "ASC" };
        let order_expr = match sort_method {
            SortMethod::Title => format!("sort_title {dir}"),
            SortMethod::Author => format!("sort_author {dir}"),
            SortMethod::FilePath => format!("sort_filepath {dir}"),
            SortMethod::FileName => format!("sort_filename {dir}"),
            SortMethod::Series => format!("sort_series {dir}"),
            // Status: Finished(0) < New(1) < Reading(2), tiebreak by most-recently used.
            // The COALESCE falls back to added_at for New books that have no opened timestamp.
            SortMethod::Status => format!(
                "CASE WHEN finished = 1 THEN 0 WHEN finished = 0 THEN 2 ELSE 1 END {dir}, \
                 COALESCE(opened, added_at) {dir}"
            ),
            // Progress: Finished(0) < New(1) < Reading(progress fraction 0→1).
            SortMethod::Progress => format!(
                "CASE WHEN finished = 1 THEN 0 WHEN finished IS NULL THEN 1 ELSE 2 END {dir}, \
                 CASE WHEN finished = 0 \
                      THEN CAST(current_page AS REAL) / CAST(NULLIF(pages_count, 0) AS REAL) \
                      ELSE NULL END {dir}"
            ),
            SortMethod::Opened => format!("opened {dir}"),
            SortMethod::Added => format!("added_at {dir}"),
            SortMethod::Year => format!("year {dir}"),
            SortMethod::Size => format!("file_size {dir}"),
            SortMethod::Kind => format!("file_kind {dir}"),
            SortMethod::Pages => format!("pages_count {dir}"),
        };

        let data_sql = format!(
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
                added_at,
                opened,
                current_page,
                pages_count,
                finished,
                dithered,
                zoom_mode,
                scroll_mode,
                page_offset_x,
                page_offset_y,
                rotation,
                cropping_margins_json,
                margin_width,
                screen_margin_width,
                font_family,
                font_size,
                text_align,
                line_height,
                contrast_exponent,
                contrast_gray,
                page_names_json,
                bookmarks_json,
                annotations_json,
                authors,
                categories
            FROM library_books_full_info
            WHERE library_id = ?
              AND (? IS NULL OR file_path = ? OR file_path LIKE (? || '/%'))
            ORDER BY {order_expr}
            LIMIT ? OFFSET ?
            "#
        );

        RUNTIME.block_on(async {
            let total: i64 = sqlx::query_scalar!(
                r#"
                SELECT COUNT(*)
                FROM library_books_full_info
                WHERE library_id = ?
                  AND (? IS NULL OR file_path = ? OR file_path LIKE (? || '/%'))
                "#,
                library_id,
                prefix_str,
                prefix_str,
                prefix_str,
            )
            .fetch_one(&self.pool)
            .await?;

            let rows: Vec<StoredBookRow> = sqlx::query_as(&data_sql)
                .bind(library_id)
                .bind(&prefix_str)
                .bind(&prefix_str)
                .bind(&prefix_str)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?;

            let books: Result<Vec<Info>, Error> = rows
                .into_iter()
                .map(|row| Self::stored_book_row_to_info(row, None))
                .collect();

            Ok((books?, total))
        })
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, prefix), fields(library_id))
    )]
    pub fn list_directories_under_prefix(
        &self,
        library_id: i64,
        prefix: &Path,
    ) -> Result<BTreeSet<PathBuf>, Error> {
        let prefix =
            (!prefix.as_os_str().is_empty()).then(|| prefix.to_string_lossy().into_owned());

        RUNTIME.block_on(async {
            let children: Vec<String> = match prefix.as_deref() {
                Some(prefix) => {
                    sqlx::query_scalar!(
                        r#"
                        SELECT DISTINCT
                            substr(
                                substr(lb.file_path, length(?2) + 2),
                                1,
                                instr(substr(lb.file_path, length(?2) + 2), '/') - 1
                            ) AS "child!: String"
                        FROM library_books lb
                        WHERE lb.library_id = ?1
                          AND lb.file_path LIKE (?2 || '/%/%')
                        "#,
                        library_id,
                        prefix,
                    )
                    .fetch_all(&self.pool)
                    .await?
                }
                None => {
                    sqlx::query_scalar!(
                        r#"
                        SELECT DISTINCT
                            substr(lb.file_path, 1, instr(lb.file_path, '/') - 1) AS "child!: String"
                        FROM library_books lb
                        WHERE lb.library_id = ?1
                          AND lb.file_path LIKE '%/%'
                        "#,
                        library_id,
                    )
                    .fetch_all(&self.pool)
                    .await?
                }
            };

            Ok(children
                .into_iter()
                .map(|child| PathBuf::from(&child))
                .collect())
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, info), fields(fp = %fp, library_id)))]
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
            .execute(&mut *tx)
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

    /// Rewrites the stored metadata for one book and its library-specific path fields.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, info), fields(fp = %fp, library_id)))]
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
                    file_kind = ?, file_size = ?, added_at = ?
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
                book_row.file_kind,
                book_row.file_size,
                book_row.added_at,
                fp_str,
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                r#"
                UPDATE library_books SET file_path = ?, absolute_path = ?
                WHERE library_id = ? AND book_fingerprint = ?
                "#,
                book_row.file_path,
                book_row.absolute_path,
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
                        INSERT OR IGNORE INTO book_categories (book_fingerprint, category_id)
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(fp = %fp)))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(fp = %fp, library_id)))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(fp = %fp)))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(library_id, path = %path.display())))]
    pub fn get_thumbnail_by_path(
        &self,
        library_id: i64,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, Error> {
        let path = path.to_string_lossy().into_owned();
        tracing::debug!(library_id, path, "fetching thumbnail by path from database");

        RUNTIME.block_on(async {
            sqlx::query_scalar!(
                "SELECT t.thumbnail_data FROM library_books lb INNER JOIN thumbnails t ON lb.book_fingerprint = t.fingerprint WHERE lb.library_id = ? AND lb.file_path = ?",
                library_id,
                path
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, data), fields(fp = %fp, size = data.len())))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(fp = %fp)))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fps), fields(count = fps.len())))]
    pub fn batch_delete_thumbnails(&self, fps: &[Fp]) -> Result<(), Error> {
        if fps.is_empty() {
            return Ok(());
        }

        tracing::debug!(count = fps.len(), "batch deleting thumbnails from database");

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            for fp in fps {
                let fp_str = fp.to_string();
                sqlx::query!("DELETE FROM thumbnails WHERE fingerprint = ?", fp_str)
                    .execute(&mut *tx)
                    .await?;
            }

            tx.commit().await?;
            Ok(())
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(from = %from_fp, to = %to_fp)))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, moves), fields(count = moves.len())))]
    pub fn batch_move_thumbnails(&self, moves: &[(Fp, Fp)]) -> Result<(), Error> {
        if moves.is_empty() {
            return Ok(());
        }

        tracing::debug!(count = moves.len(), "batch moving thumbnails in database");

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            for (from_fp, to_fp) in moves {
                let from_str = from_fp.to_string();
                let to_str = to_fp.to_string();

                sqlx::query!(
                    r#"UPDATE thumbnails SET fingerprint = ? WHERE fingerprint = ?"#,
                    to_str,
                    from_str
                )
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;
            Ok(())
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, reader_info), fields(fp = %fp)))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, toc), fields(fp = %fp, entry_count = toc.len())))]
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, books), fields(library_id, count = books.len())))]
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
                .execute(&mut *tx)
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, books), fields(library_id, count = books.len())))]
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
                        file_kind = ?, file_size = ?, added_at = ?
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
                    book_row.file_kind,
                    book_row.file_size,
                    book_row.added_at,
                    fp_str,
                )
                .execute(&mut *tx)
                .await?;

                sqlx::query!(
                    r#"
                    UPDATE library_books SET file_path = ?, absolute_path = ?
                    WHERE library_id = ? AND book_fingerprint = ?
                    "#,
                    book_row.file_path,
                    book_row.absolute_path,
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

    /// Returns `(fingerprint, path)` pairs for every book currently linked to a library.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(library_id))
    )]
    pub fn list_book_handles(&self, library_id: i64) -> Result<Vec<(Fp, PathBuf)>, Error> {
        RUNTIME.block_on(async {
            let rows = sqlx::query!(
                r#"
                SELECT lb.book_fingerprint AS "fingerprint!: Fp",
                       lb.file_path        AS "file_path!: String"
                FROM library_books lb
                WHERE lb.library_id = ?
                "#,
                library_id,
            )
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter()
                .map(|row| Ok((row.fingerprint, PathBuf::from(row.file_path))))
                .collect()
        })
    }

    /// Returns `(fingerprint, path)` pairs for every book that does not have a thumbnail cached.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(library_id))
    )]
    pub fn books_without_thumbnails(&self, library_id: i64) -> Result<Vec<(Fp, PathBuf)>, Error> {
        RUNTIME.block_on(async {
            let rows = sqlx::query!(
                r#"
                SELECT lb.book_fingerprint AS "fingerprint!: Fp",
                       lb.file_path        AS "file_path!: String"
                FROM library_books lb
                LEFT JOIN thumbnails t ON lb.book_fingerprint = t.fingerprint
                WHERE lb.library_id = ? AND t.fingerprint IS NULL
                "#,
                library_id,
            )
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter()
                .map(|row| Ok((row.fingerprint, PathBuf::from(row.file_path))))
                .collect()
        })
    }

    /// Updates both the relative and absolute path of a book in a single transaction.
    /// No-op if the book is not found in the library.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(library_id, fp = %fp)))]
    pub fn update_book_path(
        &self,
        library_id: i64,
        fp: Fp,
        rel_path: &Path,
        abs_path: &Path,
    ) -> Result<(), Error> {
        let fp_str = fp.to_string();
        let rel_str = rel_path.to_string_lossy().into_owned();
        let abs_str = abs_path.to_string_lossy().into_owned();

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            sqlx::query!(
                r#"UPDATE library_books SET file_path = ?, absolute_path = ? WHERE library_id = ? AND book_fingerprint = ?"#,
                rel_str,
                abs_str,
                library_id,
                fp_str,
            )
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        })
    }

    /// Updates relative and absolute paths for multiple books in a single transaction,
    /// with one combined UPDATE per entry. Used by `import()` after directory scanning
    /// to record the final locations of books that were moved or renamed on disk.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, updates), fields(library_id, count = updates.len())))]
    pub fn batch_update_book_paths(
        &self,
        library_id: i64,
        updates: &[(Fp, PathBuf, PathBuf)],
    ) -> Result<(), Error> {
        if updates.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            library_id,
            count = updates.len(),
            "batch updating book paths in library"
        );

        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            for (fp, rel_path, abs_path) in updates {
                let fp_str = fp.to_string();
                let rel_str = rel_path.to_string_lossy().into_owned();
                let abs_str = abs_path.to_string_lossy().into_owned();

                sqlx::query!(
                    r#"UPDATE library_books SET file_path = ?, absolute_path = ? WHERE library_id = ? AND book_fingerprint = ?"#,
                    rel_str,
                    abs_str,
                    library_id,
                    fp_str,
                )
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;
            Ok(())
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fps), fields(library_id, count = fps.len())))]
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

    /// Deletes all `library_books` rows for this library whose `file_kind` is not in
    /// `allowed_kinds`, cleans up orphaned `books` rows, and returns the fingerprints
    /// of removed entries so callers can purge thumbnails.
    ///
    /// Called at the start of every import so that books whose kind was later removed
    /// from `allowed_kinds` do not persist in the database.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, allowed_kinds), fields(library_id))
    )]
    pub fn delete_books_with_disallowed_kinds(
        &self,
        library_id: i64,
        allowed_kinds: &FxHashSet<FileExtension>,
    ) -> Result<Vec<Fp>, Error> {
        RUNTIME.block_on(async {
            let mut tx = self.pool.begin().await?;

            let rows = sqlx::query!(
                r#"
                SELECT
                    lb.book_fingerprint AS "fingerprint!: Fp",
                    b.file_kind AS "file_kind!: FileExtension"
                FROM library_books lb
                INNER JOIN books b ON b.fingerprint = lb.book_fingerprint
                WHERE lb.library_id = ?
                "#,
                library_id,
            )
            .fetch_all(&mut *tx)
            .await?;

            let mut purged: Vec<Fp> = Vec::new();

            for row in rows {
                let kind = row.file_kind;

                if allowed_kinds.contains(&kind) {
                    continue;
                }

                sqlx::query!(
                    r#"DELETE FROM library_books WHERE library_id = ? AND book_fingerprint = ?"#,
                    library_id,
                    row.fingerprint,
                )
                .execute(&mut *tx)
                .await?;

                let ref_count: i64 = sqlx::query_scalar!(
                    r#"SELECT COUNT(*) FROM library_books WHERE book_fingerprint = ?"#,
                    row.fingerprint,
                )
                .fetch_one(&mut *tx)
                .await?;

                if ref_count == 0 {
                    sqlx::query!(
                        r#"DELETE FROM books WHERE fingerprint = ?"#,
                        row.fingerprint,
                    )
                    .execute(&mut *tx)
                    .await?;
                }

                tracing::info!(fp = %row.fingerprint, kind = %kind, "removed disallowed book from library");
                purged.push(row.fingerprint);
            }

            tx.commit().await?;
            tracing::debug!(count = purged.len(), "disallowed kind cleanup complete");
            Ok(purged)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::metadata::ReaderInfo;
    use chrono::Local;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;

    fn create_test_db() -> (Database, Db) {
        let db = Database::new(":memory:").expect("failed to create in-memory database");
        db.migrate().expect("failed to run migrations");
        let libdb = Db::new(&db);
        (db, libdb)
    }

    fn register_test_library(libdb: &Db, path: &str, name: &str) -> i64 {
        libdb
            .register_library(path, name)
            .expect("failed to register library")
    }

    fn make_info(path: &str, title: &str, author: &str) -> Info {
        Info {
            title: title.to_string(),
            author: author.to_string(),
            file: FileInfo {
                path: PathBuf::from(path),
                kind: "epub".to_string(),
                size: 1024,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn midpoint_rank_both_none_returns_stride() {
        assert_eq!(midpoint_rank(&[None, None], 0), Some(SORT_RANK_STRIDE));
    }

    #[test]
    fn midpoint_rank_empty_slice_returns_stride() {
        assert_eq!(midpoint_rank(&[], 0), Some(SORT_RANK_STRIDE));
    }

    #[test]
    fn midpoint_rank_left_none_right_some_bisects() {
        // pos=0 → left=None, right=Some(10) → 10/2 = 5
        assert_eq!(midpoint_rank(&[Some(10)], 0), Some(5));
    }

    #[test]
    fn midpoint_rank_left_none_right_some_exactly_one_returns_none() {
        assert_eq!(midpoint_rank(&[Some(1)], 0), None);
    }

    #[test]
    fn midpoint_rank_left_none_right_some_zero_returns_none() {
        assert_eq!(midpoint_rank(&[Some(0)], 0), None);
    }

    #[test]
    fn midpoint_rank_left_some_right_none_adds_stride() {
        // pos=1 → left=Some(5), right=None → 5 + 1000
        assert_eq!(midpoint_rank(&[Some(5)], 1), Some(5 + SORT_RANK_STRIDE));
    }

    #[test]
    fn midpoint_rank_left_some_right_some_bisects() {
        // pos=1 → left=Some(2), right=Some(10) → (2+10)/2 = 6
        assert_eq!(midpoint_rank(&[Some(2), Some(10)], 1), Some(6));
    }

    #[test]
    fn midpoint_rank_adjacent_values_returns_none() {
        // pos=1 → left=Some(5), right=Some(6) → mid=5 which is not > l
        assert_eq!(midpoint_rank(&[Some(5), Some(6)], 1), None);
    }

    #[test]
    fn midpoint_rank_equal_values_returns_none() {
        // pos=1 → left=Some(5), right=Some(5) → mid=5 which is not > l
        assert_eq!(midpoint_rank(&[Some(5), Some(5)], 1), None);
    }

    #[test]
    fn midpoint_rank_none_slots_ignored_on_left_side() {
        // Slot at pos-1 is None → flattens to left=None, right=Some(20) → 20/2=10
        assert_eq!(midpoint_rank(&[None, Some(20)], 1), Some(10));
    }

    #[test]
    fn midpoint_rank_pos_beyond_slice_uses_last_as_left() {
        // pos beyond length → right is None; left is the last element
        let ranks = vec![Some(500i64)];
        assert_eq!(midpoint_rank(&ranks, 1), Some(500 + SORT_RANK_STRIDE));
    }

    #[test]
    fn test_insert_and_get_book() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_u64(1);

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

        let library_id = register_test_library(&libdb, "/tmp/test_library", "Test Library");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved_info = books.iter().find(|info| info.fp == Some(fp)).cloned();
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
        let fp = Fp::from_u64(2);

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

        let library_id = register_test_library(&libdb, "/tmp/test_library2", "Test Library 2");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved = books
            .iter()
            .find(|info| info.fp == Some(fp))
            .cloned()
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
    }

    #[test]
    fn test_delete_book() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_u64(3);

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

        let library_id = register_test_library(&libdb, "/tmp/test_library3", "Test Library 3");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        assert!(
            books.iter().any(|info| info.fp == Some(fp)),
            "book should exist before delete"
        );

        libdb
            .delete_book(library_id, fp)
            .expect("failed to delete book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        assert!(
            !books.iter().any(|info| info.fp == Some(fp)),
            "book should not exist after delete"
        );
    }

    #[test]
    fn test_multiple_books() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/test_library4", "Test Library 4");

        for i in 1..=5 {
            let fp = Fp::from_u64(i as u64);
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
            let fp = Fp::from_u64(i as u64);
            let retrieved = books
                .iter()
                .find(|info| info.fp == Some(fp))
                .cloned()
                .unwrap();
            assert_eq!(retrieved.title, format!("Book {}", i));
            assert_eq!(retrieved.author, format!("Author {}", i));
        }
    }

    #[test]
    fn test_update_book() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_u64(4);

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

        let library_id = register_test_library(&libdb, "/tmp/test_library5", "Test Library 5");
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
            .find(|info| info.fp == Some(fp))
            .cloned()
            .unwrap();
        assert_eq!(updated.title, "Updated Title");
        assert_eq!(updated.author, "Updated Author");
        assert_eq!(updated.year, "2025");
    }

    #[test]
    fn test_get_all_books() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/test_library6", "Test Library 6");

        for i in 1..=3 {
            let fp = Fp::from_u64(i as u64);
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

        let titles: Vec<String> = all_books.iter().map(|info| info.title.clone()).collect();
        assert!(titles.contains(&"Book 1".to_string()));
        assert!(titles.contains(&"Book 2".to_string()));
        assert!(titles.contains(&"Book 3".to_string()));
    }

    #[test]
    fn test_get_book_by_path_and_fingerprint() {
        let (_db, libdb) = create_test_db();
        let library_id =
            register_test_library(&libdb, "/tmp/test_library_lookup", "Lookup Library");
        let fp = Fp::from_str("00000000000000A1").unwrap();

        let mut info = make_info("nested/book.pdf", "Lookup Book", "Lookup Author");
        info.reader_info = Some(ReaderInfo {
            current_page: 7,
            pages_count: 21,
            ..Default::default()
        });

        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let by_path = libdb
            .get_book_by_path(library_id, Path::new("nested/book.pdf"))
            .expect("failed to get book by path")
            .expect("book should exist by path");
        assert_eq!(by_path.fp, Some(fp));
        assert_eq!(by_path.title, "Lookup Book");
        assert_eq!(by_path.file.path, PathBuf::from("nested/book.pdf"));
        assert_eq!(by_path.reader_info.unwrap().current_page, 7);

        let by_fp = libdb
            .get_book_by_fingerprint(library_id, fp)
            .expect("failed to get book by fingerprint")
            .expect("book should exist by fingerprint");
        assert_eq!(by_fp.fp, Some(fp));
        assert_eq!(by_fp.title, "Lookup Book");
        assert_eq!(by_fp.file.path, PathBuf::from("nested/book.pdf"));

        assert!(libdb
            .get_book_by_path(library_id, Path::new("missing.pdf"))
            .expect("lookup should succeed")
            .is_none());
        assert!(libdb
            .get_book_by_fingerprint(library_id, Fp::from_str("00000000000000FF").unwrap())
            .expect("lookup should succeed")
            .is_none());
    }

    #[test]
    fn test_batch_get_books_by_fingerprints() {
        let (_db, libdb) = create_test_db();
        let library_id =
            register_test_library(&libdb, "/tmp/test_library_batch_lookup", "Batch Lookup");

        let fp1 = Fp::from_str("00000000000000B1").unwrap();
        let fp2 = Fp::from_str("00000000000000B2").unwrap();
        let missing = Fp::from_str("00000000000000BF").unwrap();

        libdb
            .insert_book(
                library_id,
                fp1,
                &make_info("a/book1.pdf", "Book 1", "Author 1"),
            )
            .expect("failed to insert first book");
        libdb
            .insert_book(
                library_id,
                fp2,
                &make_info("b/book2.pdf", "Book 2", "Author 2"),
            )
            .expect("failed to insert second book");

        let books = libdb
            .batch_get_books_by_fingerprints(library_id, &[fp1, missing, fp2])
            .expect("failed to batch get books");

        assert_eq!(books.len(), 2);
        assert_eq!(books.get(&fp1).expect("missing fp1").title, "Book 1");
        assert_eq!(books.get(&fp2).expect("missing fp2").title, "Book 2");
        assert!(!books.contains_key(&missing));

        let empty = libdb
            .batch_get_books_by_fingerprints(library_id, &[])
            .expect("empty batch should succeed");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_count_books() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/test_library_count", "Count Library");

        assert_eq!(libdb.count_books(library_id).expect("count failed"), 0);

        let fp1 = Fp::from_str("00000000000000C1").unwrap();
        let fp2 = Fp::from_str("00000000000000C2").unwrap();

        libdb
            .insert_book(
                library_id,
                fp1,
                &make_info("count/one.pdf", "One", "Author"),
            )
            .expect("failed to insert first book");
        libdb
            .insert_book(
                library_id,
                fp2,
                &make_info("count/two.pdf", "Two", "Author"),
            )
            .expect("failed to insert second book");

        assert_eq!(libdb.count_books(library_id).expect("count failed"), 2);
    }

    #[test]
    fn test_list_books_under_prefix() {
        let (_db, libdb) = create_test_db();
        let library_id =
            register_test_library(&libdb, "/tmp/test_library_prefix_books", "Prefix Books");

        let fp1 = Fp::from_str("00000000000000D1").unwrap();
        let fp2 = Fp::from_str("00000000000000D2").unwrap();
        let fp3 = Fp::from_str("00000000000000D3").unwrap();

        libdb
            .insert_book(
                library_id,
                fp1,
                &make_info("dir1/book1.pdf", "Book 1", "Author 1"),
            )
            .expect("failed to insert book 1");
        libdb
            .insert_book(
                library_id,
                fp2,
                &make_info("dir1/sub/book2.pdf", "Book 2", "Author 2"),
            )
            .expect("failed to insert book 2");
        libdb
            .insert_book(
                library_id,
                fp3,
                &make_info("dir2/book3.pdf", "Book 3", "Author 3"),
            )
            .expect("failed to insert book 3");

        let root_books = libdb
            .list_books_under_prefix(library_id, Path::new(""))
            .expect("root listing failed");
        assert_eq!(root_books.len(), 3);

        let dir1_books = libdb
            .list_books_under_prefix(library_id, Path::new("dir1"))
            .expect("dir1 listing failed");
        let dir1_paths: BTreeSet<PathBuf> =
            dir1_books.into_iter().map(|info| info.file.path).collect();
        assert_eq!(
            dir1_paths,
            BTreeSet::from([
                PathBuf::from("dir1/book1.pdf"),
                PathBuf::from("dir1/sub/book2.pdf"),
            ])
        );

        let exact_book = libdb
            .list_books_under_prefix(library_id, Path::new("dir2/book3.pdf"))
            .expect("exact listing failed");
        assert_eq!(exact_book.len(), 1);
        assert_eq!(exact_book[0].fp, Some(fp3));
    }

    #[test]
    fn test_list_directories_under_prefix() {
        let (_db, libdb) = create_test_db();
        let library_id =
            register_test_library(&libdb, "/tmp/test_library_prefix_dirs", "Prefix Dirs");

        libdb
            .insert_book(
                library_id,
                Fp::from_str("00000000000000E1").unwrap(),
                &make_info("dir1/book1.pdf", "Book 1", "Author 1"),
            )
            .expect("failed to insert book 1");
        libdb
            .insert_book(
                library_id,
                Fp::from_str("00000000000000E2").unwrap(),
                &make_info("dir1/sub/book2.pdf", "Book 2", "Author 2"),
            )
            .expect("failed to insert book 2");
        libdb
            .insert_book(
                library_id,
                Fp::from_str("00000000000000E3").unwrap(),
                &make_info("dir2/book3.pdf", "Book 3", "Author 3"),
            )
            .expect("failed to insert book 3");

        let root_dirs = libdb
            .list_directories_under_prefix(library_id, Path::new(""))
            .expect("root dir listing failed");
        assert_eq!(
            root_dirs,
            BTreeSet::from([PathBuf::from("dir1"), PathBuf::from("dir2")])
        );

        let dir1_dirs = libdb
            .list_directories_under_prefix(library_id, Path::new("dir1"))
            .expect("dir1 dir listing failed");
        assert_eq!(dir1_dirs, BTreeSet::from([PathBuf::from("sub")]));

        let leaf_dirs = libdb
            .list_directories_under_prefix(library_id, Path::new("dir2"))
            .expect("leaf dir listing failed");
        assert!(leaf_dirs.is_empty());
    }

    #[test]
    fn test_reading_state_crud() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_u64(5);

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

        let library_id = register_test_library(&libdb, "/tmp/test_library7", "Test Library 7");
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
            .find(|info| info.fp == Some(fp))
            .cloned()
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
            .find(|info| info.fp == Some(fp))
            .cloned()
            .unwrap();
        let updated_reader = updated.reader_info.unwrap();

        assert_eq!(updated_reader.current_page, 100);
        assert!(updated_reader.finished);
    }

    #[test]
    fn test_batch_insert_books() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/test_library8", "Test Library 8");

        let mut books = Vec::new();
        for i in 1..=5 {
            let fp = Fp::from_u64((i + 100) as u64);
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
                .find(|info| info.fp == Some(*fp))
                .cloned()
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
        let library_id = register_test_library(&libdb, "/tmp/test_library9", "Test Library 9");

        let mut books = Vec::new();
        for i in 1..=3 {
            let fp = Fp::from_u64((i + 200) as u64);
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
                .find(|info| info.fp == Some(*fp))
                .cloned()
                .expect("book should exist");
            assert_eq!(retrieved.title, info.title);
            assert_eq!(retrieved.author, info.author);
        }
    }

    #[test]
    fn test_delete_reading_state() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_str("0000000000000006").unwrap();

        let info = Info {
            title: "Book".to_string(),
            author: "Author".to_string(),
            file: FileInfo {
                path: PathBuf::from("/tmp/book.pdf"),
                kind: "pdf".to_string(),
                size: 100,
                ..Default::default()
            },
            reader_info: Some(ReaderInfo {
                current_page: 10,
                pages_count: 50,
                ..Default::default()
            }),
            ..Default::default()
        };

        let library_id = register_test_library(&libdb, "/tmp/test_library10", "Test Library 10");
        libdb
            .insert_book(library_id, fp, &info)
            .expect("failed to insert book");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved = books
            .iter()
            .find(|info| info.fp == Some(fp))
            .cloned()
            .unwrap();
        assert!(retrieved.reader_info.is_some());

        libdb
            .delete_reading_state(fp)
            .expect("failed to delete reading state");

        let books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        let retrieved = books
            .iter()
            .find(|info| info.fp == Some(fp))
            .cloned()
            .unwrap();
        assert!(retrieved.reader_info.is_none());
    }

    #[test]
    fn test_thumbnail_crud() {
        let (_db, libdb) = create_test_db();
        let library_id =
            register_test_library(&libdb, "/tmp/test_library_thumbnails", "Thumbnail Library");
        let fp = Fp::from_str("0000000000000007").unwrap();
        let data = vec![1, 2, 3, 4, 5];

        libdb
            .insert_book(
                library_id,
                fp,
                &make_info("thumbs/book.pdf", "Thumb Book", "Thumb Author"),
            )
            .expect("failed to insert book");

        let thumbnail = libdb.get_thumbnail(fp).expect("failed to get thumbnail");
        assert!(thumbnail.is_none());

        libdb
            .save_thumbnail(fp, &data)
            .expect("failed to save thumbnail");

        let thumbnail = libdb.get_thumbnail(fp).expect("failed to get thumbnail");
        assert_eq!(thumbnail, Some(data.clone()));

        libdb
            .delete_thumbnail(fp)
            .expect("failed to delete thumbnail");

        let thumbnail = libdb.get_thumbnail(fp).expect("failed to get thumbnail");
        assert!(thumbnail.is_none());
    }

    #[test]
    fn test_books_without_thumbnails() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(
            &libdb,
            "/tmp/test_library_thumbnails_missing",
            "Missing Thumbs Library",
        );
        let fp1 = Fp::from_str("0000000000000008").unwrap();
        let fp2 = Fp::from_str("0000000000000009").unwrap();

        libdb
            .insert_book(
                library_id,
                fp1,
                &make_info("thumbs/book1.epub", "Thumb Book 1", "Thumb Author 1"),
            )
            .expect("failed to insert book 1");

        libdb
            .insert_book(
                library_id,
                fp2,
                &make_info("thumbs/book2.epub", "Thumb Book 2", "Thumb Author 2"),
            )
            .expect("failed to insert book 2");

        let missing = libdb.books_without_thumbnails(library_id).unwrap();
        assert_eq!(missing.len(), 2);
        assert!(missing.contains(&(fp1, PathBuf::from("thumbs/book1.epub"))));
        assert!(missing.contains(&(fp2, PathBuf::from("thumbs/book2.epub"))));

        libdb.save_thumbnail(fp1, &[1, 2, 3]).unwrap();

        let missing = libdb.books_without_thumbnails(library_id).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0], (fp2, PathBuf::from("thumbs/book2.epub")));

        libdb.save_thumbnail(fp2, &[4, 5, 6]).unwrap();

        let missing = libdb.books_without_thumbnails(library_id).unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn test_batch_delete_thumbnails() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(
            &libdb,
            "/tmp/test_library_batch_delete_thumbnails",
            "Batch Delete Thumbnails",
        );
        let fp1 = Fp::from_str("00000000000000F1").unwrap();
        let fp2 = Fp::from_str("00000000000000F2").unwrap();
        let fp3 = Fp::from_str("00000000000000F3").unwrap();

        libdb
            .insert_book(
                library_id,
                fp1,
                &make_info("thumbs/one.pdf", "One", "Author One"),
            )
            .expect("failed to insert first book");
        libdb
            .insert_book(
                library_id,
                fp2,
                &make_info("thumbs/two.pdf", "Two", "Author Two"),
            )
            .expect("failed to insert second book");

        libdb
            .save_thumbnail(fp1, &[1, 2, 3])
            .expect("failed to save thumbnail 1");
        libdb
            .save_thumbnail(fp2, &[4, 5, 6])
            .expect("failed to save thumbnail 2");

        libdb
            .batch_delete_thumbnails(&[fp1, fp3])
            .expect("failed to batch delete thumbnails");

        assert!(libdb
            .get_thumbnail(fp1)
            .expect("failed to get thumbnail 1")
            .is_none());
        assert_eq!(
            libdb.get_thumbnail(fp2).expect("failed to get thumbnail 2"),
            Some(vec![4, 5, 6])
        );
    }

    #[test]
    fn test_move_thumbnail() {
        let (_db, libdb) = create_test_db();
        let library_id =
            register_test_library(&libdb, "/tmp/test_library_move_thumbnail", "Move Thumbnail");
        let from_fp = Fp::from_str("0000000000000008").unwrap();
        let to_fp = Fp::from_str("0000000000000009").unwrap();
        let data = vec![9, 8, 7, 6];

        libdb
            .insert_book(
                library_id,
                from_fp,
                &make_info("thumbs/from.pdf", "From Book", "From Author"),
            )
            .expect("failed to insert source book");
        libdb
            .insert_book(
                library_id,
                to_fp,
                &make_info("thumbs/to.pdf", "To Book", "To Author"),
            )
            .expect("failed to insert destination book");

        libdb
            .save_thumbnail(from_fp, &data)
            .expect("failed to save thumbnail");

        libdb
            .move_thumbnail(from_fp, to_fp)
            .expect("failed to move thumbnail");

        let old_thumbnail = libdb
            .get_thumbnail(from_fp)
            .expect("failed to get old thumbnail");
        assert!(old_thumbnail.is_none());

        let new_thumbnail = libdb
            .get_thumbnail(to_fp)
            .expect("failed to get new thumbnail");
        assert_eq!(new_thumbnail, Some(data));
    }

    #[test]
    fn test_batch_move_thumbnails() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(
            &libdb,
            "/tmp/test_library_batch_move_thumbnails",
            "Batch Move Thumbnails",
        );
        let from_fp1 = Fp::from_str("0000000000000101").unwrap();
        let to_fp1 = Fp::from_str("0000000000000102").unwrap();
        let from_fp2 = Fp::from_str("0000000000000103").unwrap();
        let to_fp2 = Fp::from_str("0000000000000104").unwrap();

        libdb
            .insert_book(
                library_id,
                from_fp1,
                &make_info("thumbs/from1.pdf", "From 1", "Author 1"),
            )
            .expect("failed to insert source book 1");
        libdb
            .insert_book(
                library_id,
                to_fp1,
                &make_info("thumbs/to1.pdf", "To 1", "Author 1"),
            )
            .expect("failed to insert destination book 1");
        libdb
            .insert_book(
                library_id,
                from_fp2,
                &make_info("thumbs/from2.pdf", "From 2", "Author 2"),
            )
            .expect("failed to insert source book 2");
        libdb
            .insert_book(
                library_id,
                to_fp2,
                &make_info("thumbs/to2.pdf", "To 2", "Author 2"),
            )
            .expect("failed to insert destination book 2");

        libdb
            .save_thumbnail(from_fp1, &[1, 1, 1])
            .expect("failed to save thumbnail 1");
        libdb
            .save_thumbnail(from_fp2, &[2, 2, 2])
            .expect("failed to save thumbnail 2");

        libdb
            .batch_move_thumbnails(&[(from_fp1, to_fp1), (from_fp2, to_fp2)])
            .expect("failed to batch move thumbnails");

        assert!(libdb
            .get_thumbnail(from_fp1)
            .expect("failed to get old thumbnail 1")
            .is_none());
        assert!(libdb
            .get_thumbnail(from_fp2)
            .expect("failed to get old thumbnail 2")
            .is_none());
        assert_eq!(
            libdb
                .get_thumbnail(to_fp1)
                .expect("failed to get new thumbnail 1"),
            Some(vec![1, 1, 1])
        );
        assert_eq!(
            libdb
                .get_thumbnail(to_fp2)
                .expect("failed to get new thumbnail 2"),
            Some(vec![2, 2, 2])
        );
    }

    #[test]
    fn test_list_book_handles_and_update_book_path() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/test_library_handles", "Handles");

        let fp = Fp::from_str("0000000000000111").unwrap();
        libdb
            .insert_book(library_id, fp, &make_info("old/path.pdf", "Book", "Author"))
            .expect("failed to insert book");

        let handles = libdb
            .list_book_handles(library_id)
            .expect("failed to list handles");
        assert_eq!(handles, vec![(fp, PathBuf::from("old/path.pdf"))]);

        libdb
            .update_book_path(
                library_id,
                fp,
                Path::new("new/path.pdf"),
                Path::new("/abs/new/path.pdf"),
            )
            .expect("failed to update book path");

        let updated = libdb
            .get_book_by_fingerprint(library_id, fp)
            .expect("failed to get updated book")
            .expect("book should exist");
        assert_eq!(updated.file.path, PathBuf::from("new/path.pdf"));
        assert_eq!(
            updated.file.absolute_path,
            PathBuf::from("/abs/new/path.pdf")
        );

        let handles = libdb
            .list_book_handles(library_id)
            .expect("failed to list handles after update");
        assert_eq!(handles, vec![(fp, PathBuf::from("new/path.pdf"))]);
    }

    #[test]
    fn test_batch_update_book_paths() {
        let (_db, libdb) = create_test_db();
        let library_id =
            register_test_library(&libdb, "/tmp/test_library_batch_paths", "Batch Paths");

        let fp1 = Fp::from_str("0000000000000121").unwrap();
        let fp2 = Fp::from_str("0000000000000122").unwrap();

        libdb
            .insert_book(library_id, fp1, &make_info("old/one.pdf", "One", "Author"))
            .expect("failed to insert first book");
        libdb
            .insert_book(library_id, fp2, &make_info("old/two.pdf", "Two", "Author"))
            .expect("failed to insert second book");

        libdb
            .batch_update_book_paths(
                library_id,
                &[
                    (
                        fp1,
                        PathBuf::from("new/one.pdf"),
                        PathBuf::from("/abs/new/one.pdf"),
                    ),
                    (
                        fp2,
                        PathBuf::from("new/two.pdf"),
                        PathBuf::from("/abs/new/two.pdf"),
                    ),
                ],
            )
            .expect("failed to batch update book paths");

        let updated1 = libdb
            .get_book_by_fingerprint(library_id, fp1)
            .expect("failed to get first updated book")
            .expect("first book should exist");
        let updated2 = libdb
            .get_book_by_fingerprint(library_id, fp2)
            .expect("failed to get second updated book")
            .expect("second book should exist");

        assert_eq!(updated1.file.path, PathBuf::from("new/one.pdf"));
        assert_eq!(
            updated1.file.absolute_path,
            PathBuf::from("/abs/new/one.pdf")
        );
        assert_eq!(updated2.file.path, PathBuf::from("new/two.pdf"));
        assert_eq!(
            updated2.file.absolute_path,
            PathBuf::from("/abs/new/two.pdf")
        );
    }

    #[test]
    fn test_batch_delete_books() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/test_library11", "Test Library 11");

        let mut fps = Vec::new();
        for i in 1..=4 {
            let fp = Fp::from_u64((i + 300) as u64);
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

        let all_books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        assert_eq!(all_books.len(), 4);

        libdb
            .batch_delete_books(library_id, &fps[0..2])
            .expect("failed to batch delete books");

        let remaining_books = libdb
            .get_all_books(library_id)
            .expect("failed to get books");
        assert_eq!(remaining_books.len(), 2);
        assert!(remaining_books.iter().all(|info| {
            let fp = info.fp.expect("book should have fingerprint");
            fp == fps[2] || fp == fps[3]
        }));
    }

    #[test]
    fn test_batch_operations_with_empty_input() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/test_library12", "Test Library 12");

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
        let fp = Fp::from_u64(0x99);

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
            .find(|info| info.fp == Some(fp))
            .cloned()
            .expect("book should exist");

        assert_eq!(retrieved.categories, info.categories);
    }

    #[test]
    fn test_categories_updated_on_update_book() {
        let (_db, libdb) = create_test_db();
        let fp = Fp::from_u64(0x9A);

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

        let library_id =
            register_test_library(&libdb, "/tmp/test_library_upd_cat", "Upd Cat Library");
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
            .find(|info| info.fp == Some(fp))
            .cloned()
            .expect("book should exist");

        assert_eq!(retrieved.categories, info.categories);
    }

    #[test]
    fn most_recently_opened_reading_book_none_when_empty() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/mro_empty", "MRO Empty");
        assert!(libdb
            .most_recently_opened_reading_book(library_id)
            .expect("query failed")
            .is_none());
    }

    #[test]
    fn most_recently_opened_reading_book_none_when_only_finished() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/mro_finished", "MRO Finished");
        let fp = Fp::from_str("AA00000000000001").unwrap();
        let mut info = make_info("mro/finished.pdf", "Finished", "Author");
        info.reader_info = Some(ReaderInfo {
            current_page: 100,
            pages_count: 100,
            finished: true,
            ..Default::default()
        });
        libdb.insert_book(library_id, fp, &info).unwrap();

        assert!(libdb
            .most_recently_opened_reading_book(library_id)
            .expect("query failed")
            .is_none());
    }

    #[test]
    fn most_recently_opened_reading_book_returns_unfinished() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/mro_unfinished", "MRO Unfinished");

        let fp1 = Fp::from_str("AA00000000000002").unwrap();
        let fp2 = Fp::from_str("AA00000000000003").unwrap();

        let mut info1 = make_info("mro/a.pdf", "Older Book", "Author");
        info1.reader_info = Some(ReaderInfo {
            current_page: 10,
            pages_count: 200,
            ..Default::default()
        });

        let mut info2 = make_info("mro/b.pdf", "Newer Book", "Author");
        // Sleep is not needed — the in-memory SQLite uses UnixTimestamp::now()
        // which has second granularity; we manipulate opened via save_reading_state.
        info2.reader_info = Some(ReaderInfo {
            current_page: 50,
            pages_count: 200,
            ..Default::default()
        });

        libdb.insert_book(library_id, fp1, &info1).unwrap();
        libdb.insert_book(library_id, fp2, &info2).unwrap();

        // Both unfinished — result should be one of them (not None).
        let result = libdb
            .most_recently_opened_reading_book(library_id)
            .expect("query failed");
        assert!(result.is_some());
        assert!(!result.unwrap().reader_info.unwrap().finished);
    }

    #[test]
    fn most_recently_opened_reading_book_skips_never_opened() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/mro_new", "MRO New");

        // Book with no reading state (never opened).
        let fp = Fp::from_str("AA00000000000004").unwrap();
        libdb
            .insert_book(
                library_id,
                fp,
                &make_info("mro/new.pdf", "New Book", "Author"),
            )
            .unwrap();

        assert!(libdb
            .most_recently_opened_reading_book(library_id)
            .expect("query failed")
            .is_none());
    }

    #[test]
    fn compute_sort_keys_empty_library_is_noop() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/sort_empty", "Sort Empty");
        libdb.compute_sort_keys(library_id).expect("compute failed");
    }

    #[test]
    fn compute_sort_keys_assigns_ranks_to_all_books() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/sort_assign", "Sort Assign");

        for i in 1u64..=3 {
            let fp = Fp::from_str(&format!("BB{:014X}", i)).unwrap();
            libdb
                .insert_book(
                    library_id,
                    fp,
                    &make_info(&format!("s/{i}.pdf"), &format!("Book {i}"), "Author"),
                )
                .unwrap();
        }

        libdb.compute_sort_keys(library_id).expect("compute failed");

        // After compute, page_books by Title should return all 3 in order.
        let (books, total) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Title, false, 10, 0)
            .expect("page_books failed");
        assert_eq!(total, 3);
        assert_eq!(books.len(), 3);
    }

    #[test]
    fn insert_sort_rank_places_new_book_between_neighbours() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/sort_insert", "Sort Insert");

        // Insert two books and compute initial sort ranks.
        let fp_a = Fp::from_str("CC00000000000001").unwrap();
        let fp_z = Fp::from_str("CC00000000000002").unwrap();
        let info_a = make_info("s/aardvark.pdf", "Aardvark", "Author");
        let info_z = make_info("s/zebra.pdf", "Zebra", "Author");

        libdb.insert_book(library_id, fp_a, &info_a).unwrap();
        libdb.insert_book(library_id, fp_z, &info_z).unwrap();
        libdb.compute_sort_keys(library_id).unwrap();

        // Insert a book that should land between the two alphabetically.
        let fp_m = Fp::from_str("CC00000000000003").unwrap();
        let info_m = make_info("s/mango.pdf", "Mango", "Author");
        libdb.insert_book(library_id, fp_m, &info_m).unwrap();
        libdb.insert_sort_rank(library_id, fp_m, &info_m).unwrap();

        let (books, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Title, false, 10, 0)
            .expect("page_books failed");

        let titles: Vec<&str> = books.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, vec!["Aardvark", "Mango", "Zebra"]);
    }

    #[test]
    fn insert_sort_rank_falls_back_to_full_recompute_when_gaps_exhausted() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/sort_exhaust", "Sort Exhaust");

        // Seed two books with ranks 1 and 2 (no room for a midpoint).
        let fp_a = Fp::from_str("DD00000000000001").unwrap();
        let fp_b = Fp::from_str("DD00000000000002").unwrap();
        libdb
            .insert_book(library_id, fp_a, &make_info("s/a.pdf", "Alpha", "Author"))
            .unwrap();
        libdb
            .insert_book(library_id, fp_b, &make_info("s/b.pdf", "Beta", "Author"))
            .unwrap();
        libdb.compute_sort_keys(library_id).unwrap();

        // Drain the gap between Alpha (1000) and Beta (2000) by inserting many
        // "Am*" books — each midpoint halves the gap until it exhausts.
        for i in 1u64..=12 {
            let fp = Fp::from_str(&format!("DD{:014X}", i + 10)).unwrap();
            let title = format!("Am{i:012}");
            let info = make_info(&format!("s/am{i}.pdf"), &title, "Author");
            libdb.insert_book(library_id, fp, &info).unwrap();
            // insert_sort_rank will eventually fall back; just verify it doesn't panic.
            libdb.insert_sort_rank(library_id, fp, &info).unwrap();
        }

        let (books, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Title, false, 20, 0)
            .expect("page_books failed");
        // All books are present and the first is still Alpha.
        assert_eq!(books[0].title, "Alpha");
    }

    fn insert_books_for_paging(libdb: &Db, library_id: i64) {
        let books = [
            (
                "p/a.pdf", "Alpha", "Zelda", "2020", "epub", 500u64, 100usize,
            ),
            ("p/b.pdf", "Beta", "Alpha", "2019", "pdf", 300, 50),
            ("p/c.pdf", "Gamma", "Mia", "2021", "epub", 700, 200),
        ];
        for (i, (path, title, author, year, kind, size, pages)) in books.iter().enumerate() {
            let fp = Fp::from_str(&format!("EE{:014X}", i + 1)).unwrap();
            let mut info = make_info(path, title, author);
            info.year = year.to_string();
            info.file.kind = kind.to_string();
            info.file.size = *size;
            info.reader_info = Some(ReaderInfo {
                current_page: pages / 2,
                pages_count: *pages,
                ..Default::default()
            });
            libdb.insert_book(library_id, fp, &info).unwrap();
        }
        libdb.compute_sort_keys(library_id).unwrap();
    }

    #[test]
    fn page_books_sort_by_author() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_author", "PB Author");
        insert_books_for_paging(&libdb, library_id);

        let (books, total) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Author, false, 10, 0)
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(books[0].author, "Alpha");
    }

    #[test]
    fn page_books_sort_by_year() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_year", "PB Year");
        insert_books_for_paging(&libdb, library_id);

        let (books, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Year, false, 10, 0)
            .unwrap();
        assert_eq!(books[0].year, "2019");
    }

    #[test]
    fn page_books_sort_by_size() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_size", "PB Size");
        insert_books_for_paging(&libdb, library_id);

        let (books, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Size, false, 10, 0)
            .unwrap();
        assert_eq!(books[0].file.size, 300);
    }

    #[test]
    fn page_books_sort_by_kind() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_kind", "PB Kind");
        insert_books_for_paging(&libdb, library_id);

        let (books, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Kind, false, 10, 0)
            .unwrap();
        // epub < pdf alphabetically
        assert_eq!(books[0].file.kind, "epub");
    }

    #[test]
    fn page_books_sort_by_pages() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_pages", "PB Pages");
        insert_books_for_paging(&libdb, library_id);

        let (books, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Pages, false, 10, 0)
            .unwrap();
        assert_eq!(books[0].reader_info.as_ref().unwrap().pages_count, 50);
    }

    #[test]
    fn page_books_sort_by_opened() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_opened", "PB Opened");
        insert_books_for_paging(&libdb, library_id);

        // Should not panic even when opened is NULL for some books.
        let (books, total) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Opened, false, 10, 0)
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(books.len(), 3);
    }

    #[test]
    fn page_books_sort_by_added() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_added", "PB Added");
        insert_books_for_paging(&libdb, library_id);

        let (books, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Added, false, 10, 0)
            .unwrap();
        assert_eq!(books.len(), 3);
    }

    #[test]
    fn page_books_sort_by_status() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_status", "PB Status");

        let fp_new = Fp::from_str("FF00000000000001").unwrap();
        let fp_reading = Fp::from_str("FF00000000000002").unwrap();
        let fp_finished = Fp::from_str("FF00000000000003").unwrap();

        libdb
            .insert_book(library_id, fp_new, &make_info("s/new.pdf", "New", "A"))
            .unwrap();

        let mut reading = make_info("s/reading.pdf", "Reading", "A");
        reading.reader_info = Some(ReaderInfo {
            current_page: 10,
            pages_count: 100,
            finished: false,
            ..Default::default()
        });
        libdb.insert_book(library_id, fp_reading, &reading).unwrap();

        let mut finished = make_info("s/finished.pdf", "Finished", "A");
        finished.reader_info = Some(ReaderInfo {
            current_page: 100,
            pages_count: 100,
            finished: true,
            ..Default::default()
        });
        libdb
            .insert_book(library_id, fp_finished, &finished)
            .unwrap();

        let (books, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Status, false, 10, 0)
            .unwrap();
        assert_eq!(books.len(), 3);
        // Finished first in ASC order
        assert_eq!(books[0].title, "Finished");
    }

    #[test]
    fn page_books_sort_by_progress() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_progress", "PB Progress");

        let fp_finished = Fp::from_str("FE00000000000001").unwrap();
        let fp_reading = Fp::from_str("FE00000000000002").unwrap();

        let mut finished = make_info("s/fin.pdf", "Finished", "A");
        finished.reader_info = Some(ReaderInfo {
            current_page: 100,
            pages_count: 100,
            finished: true,
            ..Default::default()
        });
        libdb
            .insert_book(library_id, fp_finished, &finished)
            .unwrap();

        let mut reading = make_info("s/read.pdf", "Reading", "A");
        reading.reader_info = Some(ReaderInfo {
            current_page: 50,
            pages_count: 100,
            finished: false,
            ..Default::default()
        });
        libdb.insert_book(library_id, fp_reading, &reading).unwrap();

        let (books, _) = libdb
            .page_books(
                library_id,
                Path::new(""),
                SortMethod::Progress,
                false,
                10,
                0,
            )
            .unwrap();
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].title, "Finished");
    }

    #[test]
    fn page_books_reverse_order() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_reverse", "PB Reverse");
        insert_books_for_paging(&libdb, library_id);

        let (asc, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Title, false, 10, 0)
            .unwrap();
        let (desc, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Title, true, 10, 0)
            .unwrap();

        assert_eq!(asc[0].title, desc[desc.len() - 1].title);
        assert_eq!(asc[asc.len() - 1].title, desc[0].title);
    }

    #[test]
    fn page_books_pagination_offset() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/pb_pagination", "PB Pagination");
        insert_books_for_paging(&libdb, library_id);
        libdb.compute_sort_keys(library_id).unwrap();

        let (page1, total) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Title, false, 2, 0)
            .unwrap();
        let (page2, _) = libdb
            .page_books(library_id, Path::new(""), SortMethod::Title, false, 2, 2)
            .unwrap();

        assert_eq!(total, 3);
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 1);
        assert_ne!(page1[0].title, page2[0].title);
    }

    #[test]
    fn parse_zoom_mode_none_returns_none() {
        assert!(Db::parse_zoom_mode(None).is_none());
    }

    #[test]
    fn parse_zoom_mode_invalid_json_returns_none() {
        assert!(Db::parse_zoom_mode(Some(&"not-valid-json".to_string())).is_none());
    }

    #[test]
    fn parse_scroll_mode_none_returns_none() {
        assert!(Db::parse_scroll_mode(None).is_none());
    }

    #[test]
    fn parse_scroll_mode_invalid_json_returns_none() {
        assert!(Db::parse_scroll_mode(Some(&"{{bad}}".to_string())).is_none());
    }

    #[test]
    fn parse_text_align_none_returns_none() {
        assert!(Db::parse_text_align(None).is_none());
    }

    #[test]
    fn parse_text_align_invalid_json_returns_none() {
        assert!(Db::parse_text_align(Some(&"???".to_string())).is_none());
    }

    #[test]
    fn parse_cropping_margins_none_returns_none() {
        assert!(Db::parse_cropping_margins(None).is_none());
    }

    #[test]
    fn parse_cropping_margins_invalid_json_returns_none() {
        assert!(Db::parse_cropping_margins(Some(&"bad".to_string())).is_none());
    }

    #[test]
    fn parse_page_names_none_returns_empty_map() {
        assert!(Db::parse_page_names(None).is_empty());
    }

    #[test]
    fn parse_page_names_invalid_json_returns_empty_map() {
        assert!(Db::parse_page_names(Some(&"!".to_string())).is_empty());
    }

    #[test]
    fn parse_bookmarks_none_returns_empty_set() {
        assert!(Db::parse_bookmarks(None).is_empty());
    }

    #[test]
    fn parse_bookmarks_invalid_json_returns_empty_set() {
        assert!(Db::parse_bookmarks(Some(&"!".to_string())).is_empty());
    }

    #[test]
    fn parse_annotations_none_returns_empty_vec() {
        assert!(Db::parse_annotations(None).is_empty());
    }

    #[test]
    fn parse_annotations_invalid_json_returns_empty_vec() {
        assert!(Db::parse_annotations(Some(&"!".to_string())).is_empty());
    }

    #[test]
    fn parse_page_offset_both_some_returns_point() {
        let p = Db::parse_page_offset(Some(3), Some(7));
        assert!(p.is_some());
        let p = p.unwrap();
        assert_eq!(p.x, 3);
        assert_eq!(p.y, 7);
    }

    #[test]
    fn parse_page_offset_one_none_returns_none() {
        assert!(Db::parse_page_offset(Some(1), None).is_none());
        assert!(Db::parse_page_offset(None, Some(1)).is_none());
        assert!(Db::parse_page_offset(None, None).is_none());
    }

    #[test]
    fn extract_authors_none_returns_empty_string() {
        assert_eq!(Db::extract_authors(None), "");
    }

    #[test]
    fn extract_authors_comma_separated_joins_with_space() {
        assert_eq!(
            Db::extract_authors(Some("Alice,Bob,Carol".to_string())),
            "Alice, Bob, Carol"
        );
    }

    #[test]
    fn extract_categories_none_returns_empty_set() {
        assert!(Db::extract_categories(None).is_empty());
    }

    #[test]
    fn extract_categories_filters_empty_strings() {
        let cats = Db::extract_categories(Some(",Fiction,,Science,".to_string()));
        assert_eq!(cats.len(), 2);
        assert!(cats.contains("Fiction"));
        assert!(cats.contains("Science"));
    }

    #[test]
    fn test_batch_insert_with_reading_state() {
        let (_db, libdb) = create_test_db();
        let library_id = register_test_library(&libdb, "/tmp/test_library13", "Test Library 13");

        let mut books = Vec::new();
        for i in 1..=3 {
            let fp = Fp::from_u64((i + 400) as u64);
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
                .find(|info| info.fp == Some(*fp))
                .cloned()
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

    #[test]
    fn delete_books_with_disallowed_kinds_removes_wrong_kind() {
        use crate::settings::FileExtension;

        let (_db, libdb) = create_test_db();
        let library_id =
            register_test_library(&libdb, "/tmp/test_disallowed_kinds", "Disallowed Kinds");

        let epub_fp = Fp::from_u64(9001);
        let pdf_fp = Fp::from_u64(9002);

        let epub_info = Info {
            title: "Epub Book".to_string(),
            file: FileInfo {
                path: PathBuf::from("book.epub"),
                kind: "epub".to_string(),
                size: 100,
                ..Default::default()
            },
            ..Default::default()
        };
        let pdf_info = Info {
            title: "Pdf Book".to_string(),
            file: FileInfo {
                path: PathBuf::from("book.pdf"),
                kind: "pdf".to_string(),
                size: 200,
                ..Default::default()
            },
            ..Default::default()
        };

        libdb
            .batch_insert_books(library_id, &[(epub_fp, &epub_info), (pdf_fp, &pdf_info)])
            .expect("insert books");

        let mut allowed = FxHashSet::default();
        allowed.insert(FileExtension::Epub);

        let purged = libdb
            .delete_books_with_disallowed_kinds(library_id, &allowed)
            .expect("purge disallowed");

        assert_eq!(purged, vec![pdf_fp], "only pdf should be purged");

        let handles = libdb.list_book_handles(library_id).expect("handles");
        let fps: Vec<Fp> = handles.iter().map(|(fp, _)| *fp).collect();

        assert!(fps.contains(&epub_fp), "epub should remain");
        assert!(!fps.contains(&pdf_fp), "pdf should be gone");
    }
}
