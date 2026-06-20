//! Background task that reads `.index` files from disk and inserts their
//! entries into SQLite for fast lookups.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::num::NonZeroU64;
use std::sync::mpsc::Sender;

use globset::Glob;
use walkdir::WalkDir;

use crate::context::DICTIONARIES_DIRNAME;
use crate::db::Database;
use crate::db::runtime::RUNTIME;
use crate::device::CURRENT_DEVICE;
use crate::dictionary::{Entry, Metadata, normalize};
use crate::fl;
use crate::helpers::{Fingerprint, IsHidden};
use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::view::notification::NotificationEvent;
use crate::view::{Event, ID_FEEDER, ViewId};

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
}

impl DictionaryIndexTask {
    /// Creates a new [`DictionaryIndexTask`].
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    /// Detects dictionary metadata by scanning the first lines of the `.index`
    /// file for `00-database-allchars` and `00-database-case-sensitive` entries.
    ///
    /// Returns `(case_sensitive, all_chars)`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = %path_str)))]
    fn detect_metadata(path_str: &str) -> (bool, bool) {
        let file = match File::open(path_str) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(path = %path_str, error = %e, "failed to open index file for metadata detection");
                return (false, false);
            }
        };

        let mut all_chars = false;
        let mut case_sensitive = false;

        for line in BufReader::new(file).lines() {
            let line = match line {
                Ok(l) => l,
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
    fn resolve_index_state(
        &self,
        index_path: &std::path::Path,
        path_str: &str,
        fp_str: &str,
    ) -> Option<(i64, u64, u64, bool)> {
        let pool = self.database.pool().clone();

        let meta = RUNTIME.block_on(async {
            sqlx::query!(
                r#"SELECT dict_id, total_lines, indexed_lines, completed
                   FROM dictionary_index_meta
                   WHERE fingerprint = ?"#,
                fp_str,
            )
            .fetch_optional(&pool)
            .await
        });

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
                row.dict_id?,
                row.indexed_lines as u64,
                row.total_lines as u64,
                false,
            ));
        }

        let file = match File::open(index_path) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(path = %path_str, error = %e, "failed to open index file for line count");
                return None;
            }
        };

        let total = BufReader::new(file).lines().count() as i64;

        let result = RUNTIME.block_on(async {
            sqlx::query!(
                r#"INSERT INTO dictionary_index_meta (fingerprint, dict_path, total_lines, indexed_lines, completed)
                   VALUES (?, ?, ?, 0, 0)"#,
                fp_str,
                path_str,
                total,
            )
            .execute(&pool)
            .await
        });

        if let Err(e) = result {
            tracing::error!(path = %path_str, error = %e, "failed to insert dictionary_index_meta row");
            return None;
        }

        let dict_id: i64 = RUNTIME.block_on(async {
            sqlx::query_scalar!(
                "SELECT dict_id FROM dictionary_index_meta WHERE fingerprint = ?",
                fp_str
            )
            .fetch_one(&pool)
            .await
            .ok()?
        })?;

        Some((dict_id, 0u64, total as u64, true))
    }

    /// Marks the dictionary as fully indexed in the metadata table.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(path = %path_str, dict_id, indexed = current_line, total = total_lines)))]
    fn mark_completed(&self, dict_id: i64, path_str: &str, current_line: u64, total_lines: u64) {
        let pool = self.database.pool().clone();

        let result = RUNTIME.block_on(async {
            sqlx::query!(
                "UPDATE dictionary_index_meta SET completed = 1 WHERE dict_id = ?",
                dict_id,
            )
            .execute(&pool)
            .await
        });

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
    fn scan_and_batch(
        &self,
        job: &IndexFileJob<'_>,
        skip_lines: u64,
        hub: &Sender<Event>,
        shutdown: &ShutdownSignal,
    ) -> Option<u64> {
        let file = match File::open(job.index_path) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(path = %job.path_str, error = %e, "failed to open index file");
                return None;
            }
        };

        let reader = BufReader::new(file);
        let mut lines_iter = reader.lines().enumerate();

        for _ in 0..skip_lines {
            lines_iter.next();
        }

        let mut current_line = skip_lines;
        let mut raw_batch: Vec<Entry> = Vec::with_capacity(BATCH_SIZE);

        for (_, line_result) in &mut lines_iter {
            let line = match line_result {
                Ok(l) => l,
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

                if let Err(e) = self.flush_batch(job, &batch, current_line, hub) {
                    tracing::error!(path = %job.path_str, error = %e, "failed to flush batch");
                    return None;
                }

                raw_batch.clear();

                if shutdown.should_stop() {
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

            if let Err(e) = self.flush_batch(job, &batch, current_line, hub) {
                tracing::error!(path = %job.path_str, error = %e, "failed to flush final batch");
                return None;
            }
        }

        Some(current_line)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = %index_path.display())))]
    fn index_file(
        &self,
        index_path: &std::path::Path,
        hub: &Sender<Event>,
        shutdown: &ShutdownSignal,
    ) {
        let path_str = index_path.display().to_string();

        let dict_name = index_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_str.clone());

        let fp = match index_path.fingerprint() {
            Ok(fp) => fp,
            Err(e) => {
                tracing::error!(path = %path_str, error = %e, "failed to fingerprint index file");
                return;
            }
        };

        let fp_str = fp.to_string();

        let (dict_id, skip_lines, total_lines, is_new) =
            match self.resolve_index_state(index_path, &path_str, &fp_str) {
                Some(state) => state,
                None => {
                    return;
                }
            };

        if is_new {
            hub.send(Event::ReloadDictionaries).ok();
        }

        let (case_sensitive, all_chars) = Self::detect_metadata(&path_str);
        let metadata = Metadata {
            case_sensitive,
            all_chars,
        };

        let notif_id = ViewId::MessageNotif(ID_FEEDER.next());
        hub.send(Event::Notification(NotificationEvent::ShowPinned(
            notif_id,
            fl!(
                "notification-dictionary-indexing",
                name = dict_name.as_str()
            ),
        )))
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

        match self.scan_and_batch(&job, skip_lines, hub, shutdown) {
            Some(current_line) => {
                self.mark_completed(dict_id, &path_str, current_line, total_lines);
                hub.send(Event::ReloadDictionaries).ok();
                hub.send(Event::Close(notif_id)).ok();
            }
            None => {
                hub.send(Event::Close(notif_id)).ok();
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(batch_size = batch.len(), current_line, total_lines = job.total_lines)))]
    fn flush_batch(
        &self,
        job: &IndexFileJob<'_>,
        batch: &[(i64, String, i64, i64, Option<String>)],
        current_line: u64,
        hub: &Sender<Event>,
    ) -> Result<(), anyhow::Error> {
        let pool = self.database.pool().clone();
        let indexed_lines = current_line as i64;

        RUNTIME.block_on(async {
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

            Ok::<_, anyhow::Error>(())
        })?;

        let progress = NonZeroU64::new(job.total_lines)
            .and_then(|total_lines| {
                current_line
                    .checked_mul(100)
                    .map(|value| value / total_lines.get())
            })
            .unwrap_or(0)
            .min(100) as u8;
        let msg = fl!("notification-dictionary-indexing", name = job.dict_name);
        hub.send(Event::Notification(NotificationEvent::UpdateText(
            job.notif_id,
            msg,
        )))
        .ok();
        hub.send(Event::Notification(NotificationEvent::UpdateProgress(
            job.notif_id,
            progress,
        )))
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
    fn delete_stale_entries(
        &self,
        on_disk_fingerprints: &[String],
        hub: &Sender<Event>,
        shutdown: &ShutdownSignal,
    ) {
        let pool = self.database.pool().clone();

        let result = RUNTIME.block_on(async {
            let on_disk_set: HashSet<&str> =
                on_disk_fingerprints.iter().map(|s| s.as_str()).collect();

            let db_entries = sqlx::query!(
                "SELECT fingerprint, dict_id FROM dictionary_index_meta"
            )
            .fetch_all(&pool)
            .await?;

            let mut deleted_any = false;

            for row in db_entries {
                let fp = row.fingerprint;

                if on_disk_set.contains(fp.as_str()) {
                    continue;
                }

                let dict_id = match row.dict_id {
                    Some(id) => id,
                    None => {
                        tracing::warn!(fingerprint = %fp, "dict_id missing for stale fingerprint, skipping");
                        continue;
                    }
                };

                tracing::info!(fingerprint = %fp, "removing stale dictionary index");

                sqlx::query!(
                    "UPDATE dictionary_index_meta SET completed = 0, indexed_lines = 0 WHERE dict_id = ?",
                    dict_id,
                )
                .execute(&pool)
                .await?;

                let total_deleted =
                    delete_entries_for_dict(&pool, dict_id, shutdown).await?;

                tracing::info!(fingerprint = %fp, total_deleted, "deleted stale dictionary index entries");

                sqlx::query!(
                    "DELETE FROM dictionary_index_meta WHERE fingerprint = ?",
                    fp
                )
                .execute(&pool)
                .await?;

                deleted_any = true;

                if shutdown.should_stop() {
                    break;
                }
            }

            Ok::<_, anyhow::Error>(deleted_any)
        });

        match result {
            Ok(true) => {
                hub.send(Event::ReloadDictionaries).ok();
            }
            Ok(false) => {}
            Err(e) => {
                tracing::error!(error = %e, "failed to delete stale dictionary index entries");
            }
        }
    }
}

/// Deletes all index entries for a single dictionary in batches.
///
/// Each batch issues a single `DELETE … LIMIT` statement, keeping write locks
/// short while avoiding per-row overhead.
///
/// Returns the total number of rows deleted, or an error if any batch fails.
/// Respects the shutdown signal between batches: if a shutdown is requested
/// mid-way, the function returns early with the count deleted so far.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(pool, shutdown), fields(dict_id))
)]
async fn delete_entries_for_dict(
    pool: &sqlx::SqlitePool,
    dict_id: i64,
    shutdown: &ShutdownSignal,
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

        if shutdown.should_stop() {
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
    fn run(&mut self, hub: &Sender<Event>, shutdown: &ShutdownSignal) {
        let glob = match Glob::new("**/*.index") {
            Ok(g) => g.compile_matcher(),
            Err(e) => {
                tracing::error!(error = %e, "failed to compile glob pattern for dictionary index task");
                return;
            }
        };

        let path = CURRENT_DEVICE.data_path(DICTIONARIES_DIRNAME);

        let mut on_disk_fingerprints: Vec<String> = Vec::new();

        for entry in WalkDir::new(path)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !e.is_hidden())
        {
            if shutdown.should_stop() {
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

            if let Ok(fp) = entry.path().fingerprint() {
                on_disk_fingerprints.push(fp.to_string());
            }

            self.index_file(entry.path(), hub, shutdown);
        }

        if shutdown.should_stop() {
            return;
        }

        self.delete_stale_entries(&on_disk_fingerprints, hub, shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, runtime::RUNTIME};

    fn setup_db() -> Database {
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");
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

    #[test]
    fn test_delete_entries_for_dict_removes_all_entries() {
        let db = setup_db();
        let pool = db.pool();
        let shutdown = ShutdownSignal::never();

        RUNTIME.block_on(async {
            let dict_id = insert_meta(pool, "all-entries").await;
            for i in 0..5_i64 {
                insert_entry(pool, dict_id, "word", i).await;
            }

            let deleted = delete_entries_for_dict(pool, dict_id, &shutdown)
                .await
                .expect("delete should succeed");

            assert_eq!(deleted, 5);
            assert_eq!(count_entries(pool, dict_id).await, 0);
        });
    }

    #[test]
    fn test_delete_entries_for_dict_only_removes_target_dict() {
        let db = setup_db();
        let pool = db.pool();
        let shutdown = ShutdownSignal::never();

        RUNTIME.block_on(async {
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
        });
    }
}
