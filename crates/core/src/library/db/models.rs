use crate::db::types::{FileSize, OptionalUuid7, UnixTimestamp, Uuid7};
use crate::helpers::Fp;
use sqlx::FromRow;
use std::path::PathBuf;

/// Database row for the books table
#[derive(Debug, Clone, FromRow)]
pub struct BookRow {
    pub fingerprint: String,
    pub title: String,
    pub subtitle: String,
    pub year: String,
    pub language: String,
    pub publisher: String,
    pub series: String,
    pub edition: String,
    pub volume: String,
    pub number: String,
    pub identifier: String,
    pub file_path: String,
    pub absolute_path: String,
    pub file_kind: String,
    pub file_size: i64,
    pub added_at: UnixTimestamp,
}

/// A book record loaded from the database at the start of an import scan.
pub struct BookHandle {
    pub fp: Fp,
    pub relat: PathBuf,
    pub abs: PathBuf,
    pub mtime: Option<UnixTimestamp>,
    pub file_size: Option<FileSize>,
}

/// A pending write that records a book's new paths and current mtime/size.
///
/// Accumulated during the scan and flushed to the database in a single
/// batched transaction at the end, avoiding N individual fsyncs.
pub struct PathUpdate {
    pub fp: Fp,
    pub relat: PathBuf,
    pub abs: PathBuf,
    pub mtime: Option<UnixTimestamp>,
    pub file_size: Option<FileSize>,
}

/// Database row for the reading_states table
#[derive(Debug, Clone, FromRow)]
pub struct ReadingStateRow {
    pub fingerprint: String,
    pub opened: UnixTimestamp,
    pub current_page: i64,
    pub pages_count: i64,
    pub finished: i64,
    pub dithered: i64,
    pub zoom_mode: Option<String>,
    pub scroll_mode: Option<String>,
    pub page_offset_x: Option<i64>,
    pub page_offset_y: Option<i64>,
    pub rotation: Option<i64>,
    pub cropping_margins_json: Option<String>,
    pub margin_width: Option<i64>,
    pub screen_margin_width: Option<i64>,
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub text_align: Option<String>,
    pub line_height: Option<f64>,
    pub contrast_exponent: Option<f64>,
    pub contrast_gray: Option<f64>,
    pub page_names_json: Option<String>,
    pub bookmarks_json: Option<String>,
    pub annotations_json: Option<String>,
}

/// Database row for the toc_entries table
#[derive(Debug, Clone, FromRow)]
pub struct TocEntryRow {
    pub book_fingerprint: String,
    pub id: Uuid7,
    pub parent_id: OptionalUuid7,
    pub position: i64,
    pub title: String,
    pub location_kind: String,
    pub location_exact: Option<i64>,
    pub location_uri: Option<String>,
}
