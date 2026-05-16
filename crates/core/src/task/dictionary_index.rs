//! Background task that reads `.index` files from disk and inserts their
//! entries into SQLite for fast lookups.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;

use globset::Glob;
use walkdir::WalkDir;

use crate::context::DICTIONARIES_DIRNAME;
use crate::db::runtime::RUNTIME;
use crate::db::Database;
use crate::fl;
use crate::helpers::{Fingerprint, IsHidden};
use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::view::notification::NotificationEvent;
use crate::view::{Event, ViewId, ID_FEEDER};

const BATCH_SIZE: usize = 5000;

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

    /// Queries or initialises the metadata row for `fp_str`, returning
    /// `(skip_lines, total_lines)`.
    ///
    /// Returns `None` when the file is already fully indexed or a DB error
    /// occurs, signalling that `index_file` should skip this file.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(path = %path_str, fingerprint = %fp_str)))]
    fn resolve_index_state(
        &self,
        index_path: &std::path::Path,
        path_str: &str,
        fp_str: &str,
    ) -> Option<(u64, u64)> {
        let pool = self.database.pool().clone();

        let meta = RUNTIME.block_on(async {
            sqlx::query!(
                r#"SELECT fingerprint, total_lines, indexed_lines, completed
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

            return Some((row.indexed_lines as u64, row.total_lines as u64));
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

        Some((0u64, total as u64))
    }

    /// Marks the dictionary as fully indexed in the metadata table.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(path = %path_str, fingerprint = %fp_str, indexed = current_line, total = total_lines)))]
    fn mark_completed(&self, fp_str: &str, path_str: &str, current_line: u64, total_lines: u64) {
        let pool = self.database.pool().clone();

        let result = RUNTIME.block_on(async {
            sqlx::query!(
                "UPDATE dictionary_index_meta SET completed = 1 WHERE fingerprint = ?",
                fp_str,
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
    /// Returns `None` for metadata header lines (`00-database-*`, `00database*`)
    /// or lines that cannot be decoded. On decode failure a tracing error is
    /// emitted so the caller can skip the line without losing diagnostic info.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = %path_str)))]
    fn parse_index_line<'a>(
        path_str: &str,
        line: &'a str,
    ) -> Option<(&'a str, i64, i64, Option<&'a str>)> {
        let trimmed = line.trim_end();
        let mut cols = trimmed.split('\t');

        let word = cols.next()?;

        if word.starts_with("00-database-") || word.starts_with("00database") {
            return None;
        }

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

        let original = cols.next();

        Some((word, offset, size, original))
    }

    /// Drives the line-by-line scan of an open index file, collecting entries
    /// into batches and flushing them to the database.
    ///
    /// Returns `true` when scanning completed normally, `false` when a flush
    /// error or shutdown cut it short.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = %path_str, skip_lines, total_lines)))]
    fn scan_and_batch(
        &self,
        index_path: &std::path::Path,
        path_str: &str,
        fp_str: &str,
        dict_name: &str,
        skip_lines: u64,
        total_lines: u64,
        notif_id: ViewId,
        hub: &Sender<Event>,
        shutdown: &ShutdownSignal,
    ) -> bool {
        let file = match File::open(index_path) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(path = %path_str, error = %e, "failed to open index file");
                return false;
            }
        };

        let reader = BufReader::new(file);
        let mut lines_iter = reader.lines().enumerate();

        for _ in 0..skip_lines {
            lines_iter.next();
        }

        let mut current_line = skip_lines;
        let mut batch: Vec<(String, String, i64, i64, Option<String>)> =
            Vec::with_capacity(BATCH_SIZE);

        for (_, line_result) in &mut lines_iter {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!(path = %path_str, line = current_line, error = %e, "failed to read line");
                    current_line += 1;
                    continue;
                }
            };

            current_line += 1;

            if let Some((word, offset, size, original)) = Self::parse_index_line(path_str, &line) {
                batch.push((
                    fp_str.to_string(),
                    word.to_string(),
                    offset,
                    size,
                    original.map(String::from),
                ));
            }

            if batch.len() >= BATCH_SIZE {
                if let Err(e) = self.flush_batch(
                    &batch,
                    current_line,
                    fp_str,
                    dict_name,
                    notif_id,
                    hub,
                    total_lines,
                ) {
                    tracing::error!(path = %path_str, error = %e, "failed to flush batch");
                    return false;
                }

                batch.clear();

                if shutdown.should_stop() {
                    return false;
                }
            }
        }

        if !batch.is_empty() {
            if let Err(e) = self.flush_batch(
                &batch,
                current_line,
                fp_str,
                dict_name,
                notif_id,
                hub,
                total_lines,
            ) {
                tracing::error!(path = %path_str, error = %e, "failed to flush final batch");
                return false;
            }
        }

        true
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

        let notif_id = ViewId::MessageNotif(ID_FEEDER.next());
        hub.send(Event::Notification(NotificationEvent::ShowPinned(
            notif_id,
            fl!(
                "notification-dictionary-indexing",
                name = dict_name.as_str()
            ),
        )))
        .ok();

        let fp = match index_path.fingerprint() {
            Ok(fp) => fp,
            Err(e) => {
                tracing::error!(path = %path_str, error = %e, "failed to fingerprint index file");
                hub.send(Event::Close(notif_id)).ok();
                return;
            }
        };

        let fp_str = fp.to_string();

        let (skip_lines, total_lines) =
            match self.resolve_index_state(index_path, &path_str, &fp_str) {
                Some(state) => state,
                None => {
                    hub.send(Event::Close(notif_id)).ok();
                    return;
                }
            };

        tracing::debug!(path = %path_str, fingerprint = %fp_str, skip_lines, total_lines, "starting dictionary indexing");

        if !self.scan_and_batch(
            index_path,
            &path_str,
            &fp_str,
            &dict_name,
            skip_lines,
            total_lines,
            notif_id,
            hub,
            shutdown,
        ) {
            hub.send(Event::Close(notif_id)).ok();
            return;
        }

        self.mark_completed(&fp_str, &path_str, skip_lines + total_lines, total_lines);
        hub.send(Event::Close(notif_id)).ok();
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(batch_size = batch.len(), current_line, total_lines)))]
    fn flush_batch(
        &self,
        batch: &[(String, String, i64, i64, Option<String>)],
        current_line: u64,
        fp_str: &str,
        dict_name: &str,
        notif_id: ViewId,
        hub: &Sender<Event>,
        total_lines: u64,
    ) -> Result<(), anyhow::Error> {
        let pool = self.database.pool().clone();
        let indexed_lines = current_line as i64;

        RUNTIME.block_on(async {
            let mut tx = pool.begin().await?;

            for (fingerprint, word, offset, size, original) in batch {
                sqlx::query!(
                    r#"INSERT OR IGNORE INTO dictionary_index_entry (fingerprint, word, offset, size, original)
                       VALUES (?, ?, ?, ?, ?)"#,
                    fingerprint,
                    word,
                    offset,
                    size,
                    original,
                )
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;

            sqlx::query!(
                "UPDATE dictionary_index_meta SET indexed_lines = ? WHERE fingerprint = ?",
                indexed_lines,
                fp_str,
            )
            .execute(&pool)
            .await?;

            Ok::<_, anyhow::Error>(())
        })?;

        let progress = if total_lines > 0 {
            ((current_line * 100) / total_lines).min(100) as u8
        } else {
            0
        };
        let msg = fl!("notification-dictionary-indexing", name = dict_name);
        hub.send(Event::Notification(NotificationEvent::UpdateText(
            notif_id, msg,
        )))
        .ok();
        hub.send(Event::Notification(NotificationEvent::UpdateProgress(
            notif_id, progress,
        )))
        .ok();

        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(on_disk_count = on_disk_fingerprints.len())))]
    fn delete_stale_entries(&self, on_disk_fingerprints: &[String]) {
        let pool = self.database.pool().clone();

        let result = RUNTIME.block_on(async {
            let db_fingerprints: Vec<String> =
                sqlx::query_scalar!("SELECT fingerprint FROM dictionary_index_meta")
                    .fetch_all(&pool)
                    .await?;

            for fp in db_fingerprints {
                if !on_disk_fingerprints.contains(&fp) {
                    tracing::info!(fingerprint = %fp, "removing stale dictionary index");
                    sqlx::query!(
                        "DELETE FROM dictionary_index_meta WHERE fingerprint = ?",
                        fp
                    )
                    .execute(&pool)
                    .await?;
                }
            }

            Ok::<_, anyhow::Error>(())
        });

        if let Err(e) = result {
            tracing::error!(error = %e, "failed to delete stale dictionary index entries");
        }
    }
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

        let path = std::path::Path::new(DICTIONARIES_DIRNAME);

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

        self.delete_stale_entries(&on_disk_fingerprints);
    }
}
