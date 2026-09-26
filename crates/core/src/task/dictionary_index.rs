//! Background task that reads `.index` files from disk and inserts their
//! entries into SQLite for fast lookups.

use std::collections::HashSet;
use std::num::NonZeroU64;

use tokio::io::{AsyncBufReadExt, BufReader};

use globset::Glob;
use walkdir::WalkDir;

use crate::context::DICTIONARIES_DIRNAME;
use crate::db::Database;
use crate::device::inhibitor::{Inhibitor, Kind, SoftSuspendName};
use crate::dictionary::{Entry, Metadata, normalize};
use crate::fl;
use crate::helpers::{Fingerprint, IsHidden};
use crate::task::{BackgroundTask, TaskFuture, TaskId};
use crate::view::notification::NotificationEvent;
use crate::view::{Event, ID_FEEDER, ViewId};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const BATCH_SIZE: usize = 5000;

struct IndexFileJob<'a> {
    index_path: &'a std::path::Path,
    path_str: &'a str,
    dict_id: i64,
    dict_name: &'a str,
    total_lines: u64,
    notif_id: ViewId,
    metadata: Metadata,
}

/// Decodes a base64-like encoded number from the StarDict/dictd `.index` format.
///
/// `.index` files encode byte offsets and sizes as base-64 positional numbers
/// rather than plain integers. Each character encodes 6 bits:
///
/// | Characters | Values |
/// |------------|--------|
/// | `A`–`Z`    | 0–25   |
/// | `a`–`z`    | 26–51  |
/// | `0`–`9`    | 52–61  |
/// | `+`        | 62     |
/// | `/`        | 63     |
///
/// The decoded `u64` is a byte position (offset) or length (size) that the
/// dictionary reader uses to `seek()` directly to the right location in the
/// dictionary data file.
///
/// Returns `None` if any character falls outside the encoding alphabet.
fn decode_number(word: &str) -> Option<u64> {
    let mut index = 0u64;
    for (i, ch) in word.chars().rev().enumerate() {
        let base: u64 = match ch {
            'A'..='Z' => (ch as u64) - 65,
            'a'..='z' => (ch as u64) - 71,
            '0'..='9' => (ch as u64) + 4,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        index += base * 64u64.pow(i as u32);
    }
    Some(index)
}

/// Indexes `.index` dictionary files into SQLite for fast word lookups.
///
/// On each startup the task resumes from where it left off, so large
/// dictionaries are indexed incrementally across restarts.
pub struct DictionaryIndexTask {
    database: Database,
    data_path: std::path::PathBuf,
    inhibitor: Arc<Inhibitor>,
}

impl DictionaryIndexTask {
    /// Creates a new [`DictionaryIndexTask`].
    pub fn new(
        database: Database,
        data_path: std::path::PathBuf,
        inhibitor: Arc<Inhibitor>,
    ) -> Self {
        Self {
            database,
            data_path,
            inhibitor,
        }
    }

    /// Detects dictionary metadata by scanning the first lines of the `.index`
    /// file for `00-database-allchars` and `00-database-case-sensitive` entries.
    ///
    /// Returns `(case_sensitive, all_chars)`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = %path_str)))]
    async fn detect_metadata(path_str: &str) -> (bool, bool) {
        let file = match tokio::fs::File::open(path_str).await {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(path = %path_str, error = %e, "failed to open index file for metadata detection");
                return (false, false);
            }
        };

        let mut all_chars = false;
        let mut case_sensitive = false;
        let mut lines = BufReader::new(file).lines();

        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(_) => continue,
            };

            let word = line.split('\t').next().unwrap_or("");

            if word.is_empty() {
                continue;
            } else if word == "00-database-allchars" {
                all_chars = true;
            } else if word == "00-database-case-sensitive" || word == "00databasecasesensitive" {
                case_sensitive = true;
            } else if !word.starts_with("00-database-") && !word.starts_with("00database") {
                break;
            }

            if all_chars && case_sensitive {
                break;
            }
        }

        (case_sensitive, all_chars)
    }

    /// Queries or initialises the metadata row for `fp_str`, returning
    /// `(dict_id, skip_lines, total_lines)`.
    ///
    /// Returns `None` when the file is already fully indexed or a DB error
    /// occurs, signalling that `index_file` should skip this file.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(path = %path_str, fingerprint = %fp_str)))]
    async fn resolve_index_state(
        &self,
        index_path: &std::path::Path,
        path_str: &str,
        fp_str: &str,
    ) -> Option<(i64, u64, u64, bool)> {
        let pool = self.database.pool().clone();

        let meta = sqlx::query!(
            r#"SELECT dict_id, total_lines, indexed_lines, completed
                   FROM dictionary_index_meta
                   WHERE fingerprint = ?"#,
            fp_str,
        )
        .fetch_optional(&pool)
        .await;

        let meta = match meta {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(path = %path_str, fingerprint = %fp_str, error = %e, "failed to query dictionary_index_meta");
                return None;
            }
        };

        if let Some(row) = meta {
            if row.completed != 0 {
                tracing::debug!(path = %path_str, fingerprint = %fp_str, "dictionary already indexed, skipping");
                return None;
            }

            return Some((
                row.dict_id,
                row.indexed_lines as u64,
                row.total_lines as u64,
                false,
            ));
        }

        let file = match tokio::fs::File::open(index_path).await {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(path = %path_str, error = %e, "failed to open index file for line count");
                return None;
            }
        };

        let mut line_reader = BufReader::new(file).lines();
        let total = match count_lines(&mut line_reader).await {
            Ok(total) => total,
            Err(e) => {
                tracing::error!(
                    path = %path_str,
                    fingerprint = %fp_str,
                    error = %e,
                    "failed to read index file for line count"
                );
                return None;
            }
        };

        let result = sqlx::query!(
            r#"INSERT INTO dictionary_index_meta (fingerprint, dict_path, total_lines, indexed_lines, completed)
                   VALUES (?, ?, ?, 0, 0)"#,
            fp_str,
            path_str,
            total,
        )
        .execute(&pool)
        .await;

        if let Err(e) = result {
            tracing::error!(path = %path_str, error = %e, "failed to insert dictionary_index_meta row");
            return None;
        }

        let dict_id: i64 = sqlx::query_scalar!(
            "SELECT dict_id FROM dictionary_index_meta WHERE fingerprint = ?",
            fp_str
        )
        .fetch_one(&pool)
        .await
        .ok()?;

        Some((dict_id, 0u64, total as u64, true))
    }

    /// Marks the dictionary as fully indexed in the metadata table.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(path = %path_str, dict_id, indexed = current_line, total = total_lines)))]
    async fn mark_completed(
        &self,
        dict_id: i64,
        path_str: &str,
        current_line: u64,
        total_lines: u64,
    ) {
        let pool = self.database.pool().clone();

        let result = sqlx::query!(
            "UPDATE dictionary_index_meta SET completed = 1 WHERE dict_id = ?",
            dict_id,
        )
        .execute(&pool)
        .await;

        if let Err(e) = result {
            tracing::error!(path = %path_str, error = %e, "failed to mark dictionary as completed");
            return;
        }

        tracing::info!(path = %path_str, indexed = current_line, total = total_lines, "dictionary index complete");
    }

    /// Parses one tab-separated line from a `.index` file.
    ///
    /// Returns `None` for lines that cannot be decoded. On decode failure a
    /// tracing error is emitted so the caller can skip the line without losing
    /// diagnostic info. Metadata lines such as `00-database-*` are parsed
    /// normally so they are indexed and available for dictionary metadata
    /// queries.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = %path_str)))]
    fn parse_index_line<'a>(path_str: &str, line: &'a str) -> Option<(&'a str, i64, i64)> {
        let trimmed = line.trim_end();
        let mut cols = trimmed.split('\t');

        let word = cols.next()?;

        let offset_str = cols.next()?;
        let offset = match decode_number(offset_str) {
            Some(o) => o as i64,
            None => {
                tracing::error!(path = %path_str, word, offset_str, "failed to decode offset");
                return None;
            }
        };

        let size_str = cols.next()?;
        let size = match decode_number(size_str) {
            Some(s) => s as i64,
            None => {
                tracing::error!(path = %path_str, word, size_str, "failed to decode size");
                return None;
            }
        };

        Some((word, offset, size))
    }

    /// Drives the line-by-line scan of an open index file, collecting entries
    /// into batches and flushing them to the database.
    ///
    /// Returns `Some(current_line)` when scanning completed normally, `None`
    /// when a flush error or shutdown cut it short.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = %job.path_str, skip_lines, total_lines = job.total_lines)))]
    async fn scan_and_batch(
        &self,
        job: &IndexFileJob<'_>,
        skip_lines: u64,
        hub: &crate::view::Hub,
        shutdown: &CancellationToken,
    ) -> Option<u64> {
        let file = match tokio::fs::File::open(job.index_path).await {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(path = %job.path_str, error = %e, "failed to open index file");
                return None;
            }
        };

        let mut lines = BufReader::new(file).lines();

        for _ in 0..skip_lines {
            if let Ok(None) = lines.next_line().await {
                break;
            }
        }

        let mut current_line = skip_lines;
        let mut raw_batch: Vec<Entry> = Vec::with_capacity(BATCH_SIZE);

        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!(path = %job.path_str, line = current_line, error = %e, "failed to read line");
                    current_line += 1;
                    continue;
                }
            };

            current_line += 1;

            if let Some((word, offset, size)) = Self::parse_index_line(job.path_str, &line) {
                raw_batch.push(Entry {
                    headword: word.to_string(),
                    offset: offset as u64,
                    size: size as u64,
                    original: None,
                });
            }

            if raw_batch.len() >= BATCH_SIZE {
                let normalized = normalize(&raw_batch, &job.metadata);
                let batch: Vec<(i64, String, i64, i64, Option<String>)> = normalized
                    .into_iter()
                    .map(|e| {
                        (
                            job.dict_id,
                            e.headword,
                            e.offset as i64,
                            e.size as i64,
                            e.original,
                        )
                    })
                    .collect();

                if let Err(e) = self.flush_batch(job, &batch, current_line, hub).await {
                    tracing::error!(path = %job.path_str, error = %e, "failed to flush batch");
                    return None;
                }

                raw_batch.clear();

                if shutdown.is_cancelled() {
                    return None;
                }
            }
        }

        if !raw_batch.is_empty() {
            let normalized = normalize(&raw_batch, &job.metadata);
            let batch: Vec<(i64, String, i64, i64, Option<String>)> = normalized
                .into_iter()
                .map(|e| {
                    (
                        job.dict_id,
                        e.headword,
                        e.offset as i64,
                        e.size as i64,
                        e.original,
                    )
                })
                .collect();

            if let Err(e) = self.flush_batch(job, &batch, current_line, hub).await {
                tracing::error!(path = %job.path_str, error = %e, "failed to flush final batch");
                return None;
            }
        }

        Some(current_line)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = %index_path.display())))]
    async fn index_file(
        &self,
        index_path: &std::path::Path,
        hub: &crate::view::Hub,
        shutdown: &CancellationToken,
    ) {
        let path_str = index_path.display().to_string();

        let dict_name = index_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_str.clone());

        let index_for_hash = index_path.to_path_buf();
        let fp = match crate::runtime::spawn_blocking(move || index_for_hash.fingerprint()).await {
            Ok(Ok(fp)) => fp,
            Ok(Err(e)) => {
                tracing::error!(path = %path_str, error = %e, "failed to fingerprint index file");
                return;
            }
            Err(e) => {
                tracing::error!(path = %path_str, error = %e, "fingerprint task join failed");
                return;
            }
        };

        let fp_str = fp.to_string();

        let (dict_id, skip_lines, total_lines, is_new) = match self
            .resolve_index_state(index_path, &path_str, &fp_str)
            .await
        {
            Some(state) => state,
            None => {
                return;
            }
        };

        if is_new {
            hub.send((Event::ReloadDictionaries).into()).ok();
        }

        let (case_sensitive, all_chars) = Self::detect_metadata(&path_str).await;
        let metadata = Metadata {
            case_sensitive,
            all_chars,
        };

        let notif_id = ViewId::MessageNotif(ID_FEEDER.next());
        hub.send(
            (Event::Notification(NotificationEvent::ShowPinned(
                notif_id,
                fl!(
                    "notification-dictionary-indexing",
                    name = dict_name.as_str()
                ),
            )))
            .into(),
        )
        .ok();

        let job = IndexFileJob {
            index_path,
            path_str: &path_str,
            dict_id,
            dict_name: &dict_name,
            total_lines,
            notif_id,
            metadata,
        };

        tracing::debug!(path = %path_str, dict_id, skip_lines, total_lines, case_sensitive, all_chars, "starting dictionary indexing");

        match self.scan_and_batch(&job, skip_lines, hub, shutdown).await {
            Some(current_line) => {
                self.mark_completed(dict_id, &path_str, current_line, total_lines)
                    .await;
                hub.send((Event::ReloadDictionaries).into()).ok();
                hub.send((Event::Close(notif_id)).into()).ok();
            }
            None => {
                hub.send((Event::Close(notif_id)).into()).ok();
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(batch_size = batch.len(), current_line, total_lines = job.total_lines)))]
    async fn flush_batch(
        &self,
        job: &IndexFileJob<'_>,
        batch: &[(i64, String, i64, i64, Option<String>)],
        current_line: u64,
        hub: &crate::view::Hub,
    ) -> Result<(), anyhow::Error> {
        let pool = self.database.pool().clone();
        let indexed_lines = current_line as i64;

        let mut tx = pool.begin().await?;

        for (dict_id, word, offset, size, original) in batch {
            sqlx::query!(
                    r#"INSERT OR IGNORE INTO dictionary_index_entry (dict_id, word, offset, size, original)
                       VALUES (?, ?, ?, ?, ?)"#,
                    dict_id,
                    word,
                    offset,
                    size,
                    original,
                )
                .execute(&mut *tx)
                .await?;
        }

        sqlx::query!(
            "UPDATE dictionary_index_meta SET indexed_lines = ? WHERE dict_id = ?",
            indexed_lines,
            job.dict_id,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let progress = NonZeroU64::new(job.total_lines)
            .and_then(|total_lines| {
                current_line
                    .checked_mul(100)
                    .map(|value| value / total_lines.get())
            })
            .unwrap_or(0)
            .min(100) as u8;
        let msg = fl!("notification-dictionary-indexing", name = job.dict_name);
        hub.send((Event::Notification(NotificationEvent::UpdateText(job.notif_id, msg))).into())
            .ok();
        hub.send(
            (Event::Notification(NotificationEvent::UpdateProgress(job.notif_id, progress))).into(),
        )
        .ok();

        Ok(())
    }

    /// Removes index data for dictionaries that are no longer present on disk.
    ///
    /// For each fingerprint in `dictionary_index_meta` that has no corresponding
    /// `.index` file in `on_disk_fingerprints`, this method marks the meta row as
    /// incomplete before deletion begins. This ensures that if the process is
    /// interrupted mid-deletion, the next startup does not treat a partially
    /// deleted dictionary as fully indexed. Entries are then removed via
    /// [`delete_entries_for_dict`], after which the meta row itself is deleted.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(on_disk_count = on_disk_fingerprints.len())))]
    async fn delete_stale_entries(
        &self,
        on_disk_fingerprints: &[String],
        hub: &crate::view::Hub,
        shutdown: &CancellationToken,
    ) {
        let pool = self.database.pool().clone();
        let result = purge_stale_dictionary_indexes(&pool, on_disk_fingerprints, shutdown).await;

        match result {
            Ok(true) => {
                hub.send((Event::ReloadDictionaries).into()).ok();
            }
            Ok(false) => {}
            Err(e) => {
                tracing::error!(error = %e, "failed to delete stale dictionary index entries");
            }
        }
    }
}

async fn purge_stale_dictionary_indexes(
    pool: &sqlx::SqlitePool,
    on_disk_fingerprints: &[String],
    shutdown: &CancellationToken,
) -> anyhow::Result<bool> {
    let on_disk_set: HashSet<&str> = on_disk_fingerprints.iter().map(|s| s.as_str()).collect();

    let db_entries = sqlx::query!("SELECT fingerprint, dict_id FROM dictionary_index_meta")
        .fetch_all(pool)
        .await?;

    let mut deleted_any = false;

    for row in db_entries {
        let fp = row.fingerprint;

        if on_disk_set.contains(fp.as_str()) {
            continue;
        }

        let dict_id = row.dict_id;

        tracing::info!(fingerprint = %fp, "removing stale dictionary index");

        sqlx::query!(
            "UPDATE dictionary_index_meta SET completed = 0, indexed_lines = 0 WHERE dict_id = ?",
            dict_id,
        )
        .execute(pool)
        .await?;

        let total_deleted = delete_entries_for_dict(pool, dict_id, shutdown).await?;

        tracing::info!(fingerprint = %fp, total_deleted, "deleted stale dictionary index entries");

        sqlx::query!(
            "DELETE FROM dictionary_index_meta WHERE fingerprint = ?",
            fp
        )
        .execute(pool)
        .await?;

        deleted_any = true;

        if shutdown.is_cancelled() {
            break;
        }
    }

    Ok(deleted_any)
}

/// Counts lines from `reader`. An I/O error stops the count instead of spinning.
async fn count_lines<R>(reader: &mut tokio::io::Lines<R>) -> std::io::Result<i64>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut total = 0_i64;
    loop {
        match reader.next_line().await? {
            Some(_) => total += 1,
            None => return Ok(total),
        }
    }
}

/// Deletes all index entries for a single dictionary in batches.
///
/// Each batch issues a single `DELETE … LIMIT` statement, keeping write locks
/// short while avoiding per-row overhead.
///
/// Returns the total number of rows deleted, or an error if any batch fails.
///
/// Checks cancellation between batches. A batch that has already been deleted
/// stays deleted, and the function returns that count so a later run can
/// continue from the rows that remain.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(pool, shutdown), fields(dict_id))
)]
async fn delete_entries_for_dict(
    pool: &sqlx::SqlitePool,
    dict_id: i64,
    shutdown: &CancellationToken,
) -> Result<u64, anyhow::Error> {
    let batch_size = BATCH_SIZE as i64;
    let mut total_deleted: u64 = 0;

    loop {
        let rows_affected = sqlx::query!(
            "DELETE FROM dictionary_index_entry WHERE dict_id = ? LIMIT ?",
            dict_id,
            batch_size,
        )
        .execute(pool)
        .await?
        .rows_affected();

        if rows_affected == 0 {
            break;
        }

        total_deleted += rows_affected;

        if shutdown.is_cancelled() {
            tracing::info!(total_deleted, "entry deletion interrupted by shutdown");
            return Ok(total_deleted);
        }
    }

    Ok(total_deleted)
}

impl BackgroundTask for DictionaryIndexTask {
    fn id(&self) -> TaskId {
        TaskId::DictionaryIndex
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn run<'a>(
        &'a mut self,
        hub: &'a crate::view::Hub,
        shutdown: &'a CancellationToken,
    ) -> TaskFuture<'a> {
        Box::pin(async move {
            let _soft_suspend = match self
                .inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::DictionaryIndex)
            {
                Ok(guard) => Some(guard),
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        soft_suspend_lease = %SoftSuspendName::DictionaryIndex,
                        "failed to acquire soft-suspend lease for dictionary index task"
                    );
                    None
                }
            };
            let glob = match Glob::new("**/*.index") {
                Ok(g) => g.compile_matcher(),
                Err(e) => {
                    tracing::error!(error = %e, "failed to compile glob pattern for dictionary index task");
                    return;
                }
            };

            let path = self.data_path.join(DICTIONARIES_DIRNAME);

            if !path.is_dir() {
                tracing::warn!(
                    path = %path.display(),
                    "dictionaries directory not found, skipping index"
                );
                return;
            }

            let mut on_disk_fingerprints: Vec<String> = Vec::new();

            for entry in WalkDir::new(path)
                .min_depth(1)
                .into_iter()
                .filter_entry(|e| !e.is_hidden())
            {
                if shutdown.is_cancelled() {
                    return;
                }

                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!(error = %e, "failed to read directory entry");
                        continue;
                    }
                };

                if !glob.is_match(entry.path()) {
                    continue;
                }

                let entry_path = entry.path().to_path_buf();
                if let Ok(Ok(fp)) =
                    crate::runtime::spawn_blocking(move || entry_path.fingerprint()).await
                {
                    on_disk_fingerprints.push(fp.to_string());
                }

                self.index_file(entry.path(), hub, shutdown).await;
            }

            if shutdown.is_cancelled() {
                return;
            }

            self.delete_stale_entries(&on_disk_fingerprints, hub, shutdown)
                .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::io;
    use tokio::io::AsyncRead;

    struct FailRead;

    impl AsyncRead for FailRead {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, "eio")))
        }
    }

    #[tokio::test]
    async fn count_lines_stops_on_io_error() {
        let mut lines = BufReader::new(FailRead).lines();
        let err = count_lines(&mut lines)
            .await
            .expect_err("persistent read error");
        assert_eq!(err.kind(), io::ErrorKind::Other);
    }

    #[tokio::test]
    async fn count_lines_counts_successful_lines() {
        let mut lines = BufReader::new(&b"one\ntwo\n"[..]).lines();
        assert_eq!(count_lines(&mut lines).await.expect("read"), 2);
    }

    async fn setup_db() -> Database {
        let mut db = Database::new(":memory:")
            .await
            .expect("failed to create in-memory database");
        db.init_for_test(0).await.expect("failed to run migrations");
        db
    }

    async fn insert_meta(pool: &sqlx::SqlitePool, fingerprint: &str) -> i64 {
        sqlx::query_scalar!(
            "INSERT INTO dictionary_index_meta (fingerprint, dict_path, total_lines) VALUES (?, ?, ?) RETURNING dict_id",
            fingerprint,
            fingerprint,
            0_i64,
        )
        .fetch_one(pool)
        .await
        .expect("failed to insert meta")
    }

    async fn insert_entry(pool: &sqlx::SqlitePool, dict_id: i64, word: &str, offset: i64) {
        sqlx::query!(
            "INSERT INTO dictionary_index_entry (dict_id, word, offset, size) VALUES (?, ?, ?, 0)",
            dict_id,
            word,
            offset,
        )
        .execute(pool)
        .await
        .expect("failed to insert entry");
    }

    async fn count_entries(pool: &sqlx::SqlitePool, dict_id: i64) -> i64 {
        sqlx::query_scalar!(
            "SELECT COUNT(*) FROM dictionary_index_entry WHERE dict_id = ?",
            dict_id,
        )
        .fetch_one(pool)
        .await
        .expect("failed to count entries")
    }

    #[tokio::test]
    async fn test_delete_entries_for_dict_removes_all_entries() {
        let db = setup_db().await;
        let pool = db.pool();
        let shutdown = CancellationToken::new();

        let dict_id = insert_meta(pool, "all-entries").await;
        for i in 0..5_i64 {
            insert_entry(pool, dict_id, "word", i).await;
        }

        let deleted = delete_entries_for_dict(pool, dict_id, &shutdown)
            .await
            .expect("delete should succeed");

        assert_eq!(deleted, 5);
        assert_eq!(count_entries(pool, dict_id).await, 0);
    }

    #[tokio::test]
    async fn delete_entries_keeps_a_finished_batch_when_cancelled() {
        let db = setup_db().await;
        let pool = db.pool();
        let dict_id = insert_meta(pool, "partial").await;
        let total = BATCH_SIZE as i64 + 1;
        for i in 0..total {
            insert_entry(pool, dict_id, "word", i).await;
        }

        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let deleted = delete_entries_for_dict(pool, dict_id, &shutdown)
            .await
            .expect("delete should succeed");

        assert_eq!(deleted, BATCH_SIZE as u64);
        assert_eq!(count_entries(pool, dict_id).await, 1);
    }

    #[tokio::test]
    async fn test_delete_entries_for_dict_only_removes_target_dict() {
        let db = setup_db().await;
        let pool = db.pool();
        let shutdown = CancellationToken::new();

        let dict_a = insert_meta(pool, "dict-a").await;
        let dict_b = insert_meta(pool, "dict-b").await;

        insert_entry(pool, dict_a, "apple", 0).await;
        insert_entry(pool, dict_b, "banana", 0).await;
        insert_entry(pool, dict_b, "cherry", 0).await;

        let deleted = delete_entries_for_dict(pool, dict_a, &shutdown)
            .await
            .expect("delete should succeed");

        assert_eq!(deleted, 1);
        assert_eq!(count_entries(pool, dict_a).await, 0);
        assert_eq!(count_entries(pool, dict_b).await, 2);
    }
}
