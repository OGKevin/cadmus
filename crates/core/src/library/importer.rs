use crate::db::types::{FileSize, UnixTimestamp};
use crate::document::file_kind;
use crate::fl;
use crate::helpers::{Fingerprint, Fp, IsHidden};
use crate::library::book_status::BookStatus;
use crate::library::db::{Db as LibraryDb, ImportFlush, PathUpdate};
use crate::metadata::{FileInfo, Info, extract_metadata_from_document};
use crate::settings::ImportSettings;
use crate::view::{Event, NotificationEvent, ViewId};
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use walkdir::{DirEntry, WalkDir};

/// Result of one library import attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportOutcome {
    /// The scan finished and its results were recorded.
    Completed,
    /// Shutdown stopped the scan before results were recorded.
    Interrupted,
    /// The library could not be opened or the scan could not start.
    Failed,
}

struct PinnedProgress<'a> {
    hub: &'a crate::view::Hub,
    notif_id: ViewId,
}

impl<'a> PinnedProgress<'a> {
    fn show(hub: &'a crate::view::Hub, notif_id: ViewId, message: String) -> Self {
        hub.send((Event::Notification(NotificationEvent::ShowPinned(notif_id, message))).into())
            .ok();
        Self { hub, notif_id }
    }
}

impl Drop for PinnedProgress<'_> {
    fn drop(&mut self) {
        self.hub.send((Event::Close(self.notif_id)).into()).ok();
    }
}

enum PendingRelocation {
    FingerprintChanged {
        new_fp: Fp,
        old_fp: Fp,
        file_size: u64,
    },
}

impl PendingRelocation {
    fn old_fp(&self) -> Fp {
        match self {
            PendingRelocation::FingerprintChanged { old_fp, .. } => *old_fp,
        }
    }
}

struct ProgressTracker {
    last_sent: Instant,
    last_percent: u8,
    first_tick: bool,
}

impl ProgressTracker {
    const SEND_INTERVAL_SEC: u64 = 2;

    fn new() -> Self {
        Self {
            last_sent: Instant::now(),
            last_percent: 0,
            first_tick: true,
        }
    }

    fn should_send(&mut self, idx: usize, total: usize, now: Instant) -> Option<u8> {
        let percent = ((idx + 1) * 100).checked_div(total)?;
        let percent = percent.min(100) as u8;

        if self.first_tick
            || percent == 100
            || now.checked_duration_since(self.last_sent)
                >= Some(Duration::from_secs(Self::SEND_INTERVAL_SEC))
        {
            self.last_sent = now;
            self.last_percent = percent;
            self.first_tick = false;
            Some(percent)
        } else {
            None
        }
    }
}

struct ScanContext<'a> {
    hub: &'a crate::view::Hub,
    notif_id: ViewId,
    shutdown: &'a CancellationToken,
}

struct BookWrite {
    fp: Fp,
    info: Info,
}

struct ScanResult {
    books_to_insert: Vec<BookWrite>,
    books_to_update: Vec<BookWrite>,
    books_to_link: Vec<BookWrite>,
    path_updates: Vec<PathUpdate>,
    books_to_delete: Vec<Fp>,
    pending_relocations: Vec<PendingRelocation>,
    thumbnails_to_delete: Vec<Fp>,
}

impl ScanResult {
    fn empty() -> Self {
        Self {
            books_to_insert: Vec::new(),
            books_to_update: Vec::new(),
            books_to_link: Vec::new(),
            path_updates: Vec::new(),
            books_to_delete: Vec::new(),
            pending_relocations: Vec::new(),
            thumbnails_to_delete: Vec::new(),
        }
    }

    fn push_new_book(&mut self, existing: Option<BookStatus>, book: BookWrite) {
        match existing {
            None => self.books_to_insert.push(book),
            Some(BookStatus::PendingDiscovery) => self.books_to_update.push(book),
            Some(BookStatus::Active) => self.books_to_link.push(book),
        }
    }
}

#[cfg(feature = "emulator")]
const IGNORED_TOP_LEVEL_DIRS: &[&str] = &["target", "node_modules", "thirdparty"];

/// Reads document metadata on the blocking pool.
///
/// The document handle stays inside the blocking closure so it never crosses
/// an await. A panic returns the metadata collected before extraction.
async fn extract_metadata(home: &Path, install_dir: &Path, info: Info) -> Info {
    let fallback = info.clone();
    let home = home.to_path_buf();
    let install_dir = install_dir.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let mut info = info;
        extract_metadata_from_document(&home, &mut info, &install_dir);
        info
    })
    .await
    {
        Ok(info) => info,
        Err(error) => {
            error!(error = %error, "metadata extraction task panicked");
            fallback
        }
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(home)))]
fn walk_files(home: &Path) -> Vec<DirEntry> {
    WalkDir::new(home)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            if e.is_hidden() {
                return false;
            }
            #[cfg(feature = "emulator")]
            if e.depth() == 1 && e.file_type().is_dir() {
                if let Some(name) = e.file_name().to_str() {
                    if IGNORED_TOP_LEVEL_DIRS.contains(&name) {
                        return false;
                    }
                }
            }
            true
        })
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_type().is_dir())
        .collect()
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(
            home,
            install_dir,
            settings,
            ctx,
            tracker,
            mtime_by_abs,
            handles_by_fp,
            handles_by_path,
            pending_fps,
            book_statuses,
            entries
        ),
        fields(total)
    )
)]
#[allow(clippy::too_many_arguments)]
async fn scan_entries(
    home: &Path,
    install_dir: &Path,
    entries: &[DirEntry],
    settings: &ImportSettings,
    force: bool,
    ctx: &ScanContext<'_>,
    tracker: &mut ProgressTracker,
    mtime_by_abs: &FxHashMap<PathBuf, (UnixTimestamp, FileSize)>,
    handles_by_fp: &mut FxHashMap<Fp, (PathBuf, PathBuf)>,
    handles_by_path: &mut FxHashMap<PathBuf, Fp>,
    pending_fps: &FxHashSet<Fp>,
    book_statuses: &FxHashMap<Fp, BookStatus>,
) -> Option<ScanResult> {
    let total = entries.len();
    tracing::Span::current().record("total", total);
    let mut skipped_count = 0u32;
    let mut fingerprinted_count = 0u32;
    let mut mtime_miss_count = 0u32;
    debug!(mtime_map_size = mtime_by_abs.len(), "starting scan");

    let mut result = ScanResult::empty();

    for (idx, entry) in entries.iter().enumerate() {
        #[cfg(feature = "tracing")]
        let span = tracing::info_span!("procssing entry", entry = ?entry);
        #[cfg(feature = "tracing")]
        let mut entered = Some(span.enter());

        if ctx.shutdown.is_cancelled() {
            tracing::info!("import scan interrupted by shutdown");
            return None;
        }

        let path = entry.path();
        let relat = path.strip_prefix(home).unwrap_or(path);

        let kind = file_kind(path);
        let is_known_to_db = handles_by_path.contains_key(relat);
        let allowed_kind = kind.filter(|k| settings.is_kind_allowed(*k));
        let path_is_pending = handles_by_path
            .get(relat)
            .is_some_and(|fp| pending_fps.contains(fp));

        if !is_known_to_db && allowed_kind.is_none() {
            send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
            continue;
        }

        let file_meta = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(e) => {
                error!(path = ?path, error = %e, "failed to read metadata, skipping");
                send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
                continue;
            }
        };

        let current_size = FileSize::from(file_meta.len() as i64);
        let current_mtime = file_meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| UnixTimestamp::from((d.as_secs().div_ceil(2) * 2) as i64));

        if !force
            && !path_is_pending
            && let Some(mtime) = current_mtime
        {
            match mtime_by_abs.get(path) {
                Some(&(stored_mtime, stored_size)) => {
                    if stored_mtime == mtime && stored_size == current_size {
                        skipped_count += 1;
                        debug!(path = %relat.display(), "mtime and size unchanged, skipping fingerprint");
                        send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
                        continue;
                    }
                }
                None => {
                    mtime_miss_count += 1;
                    debug!(
                        path = %relat.display(),
                        abs = %path.display(),
                        "mtime lookup miss: file not in mtime_by_abs map"
                    );
                }
            }
        }

        let path_for_hash = path.to_path_buf();
        let fp = match crate::runtime::spawn_blocking(move || path_for_hash.fingerprint()).await {
            Ok(Ok(fp)) => {
                fingerprinted_count += 1;
                fp
            }
            Ok(Err(e)) => {
                error!(path = ?path, error = %e, "failed to compute fingerprint, skipping");
                send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
                continue;
            }
            Err(e) => {
                error!(path = ?path, error = %e, "fingerprint task join failed, skipping");
                send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
                continue;
            }
        };

        if handles_by_fp.contains_key(&fp) {
            let (stored_relat, stored_abs) = handles_by_fp[&fp].clone();
            let path_changed = relat != stored_relat;
            let abs_stale = path != stored_abs;

            if path_changed {
                debug!(
                    fp = %fp,
                    old_path = %stored_relat.display(),
                    new_path = %relat.display(),
                    "updated book path"
                );
                handles_by_path.remove(&stored_relat);
                handles_by_fp.insert(fp, (relat.to_path_buf(), path.to_path_buf()));
                handles_by_path.insert(relat.to_path_buf(), fp);
            } else if abs_stale {
                debug!(
                    fp = %fp,
                    path = %relat.display(),
                    "healing stale absolute_path"
                );
                handles_by_fp.insert(fp, (relat.to_path_buf(), path.to_path_buf()));
            }

            result.path_updates.push(PathUpdate {
                fp,
                relat: relat.to_path_buf(),
                abs: path.to_path_buf(),
                mtime: current_mtime,
                file_size: Some(current_size),
            });

            if pending_fps.contains(&fp) {
                if let Some(kind) = allowed_kind {
                    info!(fp = %fp, path = %relat.display(), "filling pending discovery book");
                    let size = i64::from(current_size) as u64;
                    let mut book_info = Info {
                        file: FileInfo {
                            path: relat.to_path_buf(),
                            absolute_path: path.to_path_buf(),
                            kind: Some(kind),
                            size,
                            mtime: current_mtime,
                        },
                        ..Default::default()
                    };
                    if settings.metadata_kinds.contains(&kind) {
                        #[cfg(feature = "tracing")]
                        {
                            entered.take();
                        }
                        book_info = extract_metadata(home, install_dir, book_info).await;
                        #[cfg(feature = "tracing")]
                        {
                            entered = Some(span.enter());
                        }
                    }
                    result.books_to_update.push(BookWrite {
                        fp,
                        info: book_info,
                    });
                } else {
                    debug!(
                        fp = %fp,
                        path = %relat.display(),
                        "pending book found but kind not allowed; leaving pending"
                    );
                }
            }

            send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
            #[cfg(feature = "tracing")]
            drop(entered);
            continue;
        }

        if let Some(old_fp) = handles_by_path.get(relat).cloned() {
            debug!(
                path = %relat.display(),
                old_fp = %old_fp,
                new_fp = %fp,
                "updated book fingerprint"
            );

            handles_by_fp.remove(&old_fp);
            handles_by_path.remove(relat);
            handles_by_fp.insert(fp, (relat.to_path_buf(), path.to_path_buf()));
            handles_by_path.insert(relat.to_path_buf(), fp);
            result.books_to_delete.push(old_fp);

            result
                .pending_relocations
                .push(PendingRelocation::FingerprintChanged {
                    new_fp: fp,
                    old_fp,
                    file_size: i64::from(current_size) as u64,
                });

            result.thumbnails_to_delete.push(old_fp);
            result.path_updates.push(PathUpdate {
                fp,
                relat: relat.to_path_buf(),
                abs: path.to_path_buf(),
                mtime: current_mtime,
                file_size: Some(current_size),
            });
            send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
            continue;
        }

        if let Some(kind) = allowed_kind {
            info!(fp = %fp, path = %relat.display(), "added new entry");
            let size = i64::from(current_size) as u64;
            let mut book_info = Info {
                file: FileInfo {
                    path: relat.to_path_buf(),
                    absolute_path: path.to_path_buf(),
                    kind: Some(kind),
                    size,
                    mtime: current_mtime,
                },
                ..Default::default()
            };
            if settings.metadata_kinds.contains(&kind) {
                #[cfg(feature = "tracing")]
                {
                    entered.take();
                }
                book_info = extract_metadata(home, install_dir, book_info).await;
                #[cfg(feature = "tracing")]
                {
                    entered = Some(span.enter());
                }
            }
            handles_by_fp.insert(fp, (relat.to_path_buf(), path.to_path_buf()));
            handles_by_path.insert(relat.to_path_buf(), fp);
            result.push_new_book(
                book_statuses.get(&fp).copied(),
                BookWrite {
                    fp,
                    info: book_info,
                },
            );
        }

        send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
        #[cfg(feature = "tracing")]
        drop(entered);
    }

    info!(
        total,
        skipped = skipped_count,
        fingerprinted = fingerprinted_count,
        mtime_misses = mtime_miss_count,
        "scan complete"
    );

    Some(result)
}

fn send_progress(
    hub: &crate::view::Hub,
    notif_id: ViewId,
    tracker: &mut ProgressTracker,
    idx: usize,
    total: usize,
) {
    let Some(percent) = tracker.should_send(idx, total, Instant::now()) else {
        return;
    };
    debug!(percent, "import progress");
    hub.send((Event::Notification(NotificationEvent::UpdateProgress(notif_id, percent))).into())
        .ok();
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(
        db,
        home,
        install_dir,
        settings,
        pending_relocations,
        result,
        book_statuses
    ))
)]
#[allow(clippy::too_many_arguments)]
async fn resolve_relocations(
    db: &LibraryDb,
    library_id: i64,
    home: &Path,
    install_dir: &Path,
    settings: &ImportSettings,
    pending_relocations: Vec<PendingRelocation>,
    result: &mut ScanResult,
    book_statuses: &FxHashMap<Fp, BookStatus>,
) {
    let old_fps: Vec<Fp> = pending_relocations
        .iter()
        .map(PendingRelocation::old_fp)
        .collect();

    let mut fetched = db
        .batch_get_books_by_fingerprints(library_id, &old_fps)
        .await
        .unwrap_or_default();

    for relocation in pending_relocations {
        match relocation {
            PendingRelocation::FingerprintChanged {
                new_fp,
                old_fp,
                file_size,
            } => {
                if let Some(mut info) = fetched.remove(&old_fp) {
                    if settings.sync_metadata
                        && info
                            .file
                            .kind
                            .is_some_and(|k| settings.metadata_kinds.contains(&k))
                    {
                        info = extract_metadata(home, install_dir, info).await;
                    }
                    info.file.size = file_size;
                    result.push_new_book(
                        book_statuses.get(&new_fp).copied(),
                        BookWrite { fp: new_fp, info },
                    );
                }
            }
        }
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(handles_by_fp, home)))]
fn find_deleted_books(handles_by_fp: &FxHashMap<Fp, (PathBuf, PathBuf)>, home: &Path) -> Vec<Fp> {
    handles_by_fp
        .iter()
        .filter(|(_, (relat, _))| relat.as_os_str().is_empty() || !home.join(relat).exists())
        .map(|(fp, (relat, _))| {
            info!(fp = %fp, path = %relat.display(), "removing deleted entry");
            *fp
        })
        .collect()
}

fn sort_keys_are_dirty(purged_fps: &[Fp], result: &ScanResult) -> bool {
    !purged_fps.is_empty()
        || !result.books_to_insert.is_empty()
        || !result.books_to_update.is_empty()
        || !result.books_to_link.is_empty()
        || !result.path_updates.is_empty()
        || !result.books_to_delete.is_empty()
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(db, result)))]
async fn flush_to_db(db: &LibraryDb, library_id: i64, result: ScanResult, purged_fps: &[Fp]) {
    let sort_keys_dirty = sort_keys_are_dirty(purged_fps, &result);
    if !sort_keys_dirty && result.thumbnails_to_delete.is_empty() {
        return;
    }

    let books_to_insert: Vec<(Fp, &Info)> = result
        .books_to_insert
        .iter()
        .map(|book| (book.fp, &book.info))
        .collect();
    let books_to_update: Vec<(Fp, &Info)> = result
        .books_to_update
        .iter()
        .map(|book| (book.fp, &book.info))
        .collect();
    let books_to_link: Vec<(Fp, &Info)> = result
        .books_to_link
        .iter()
        .map(|book| (book.fp, &book.info))
        .collect();

    if let Err(e) = db
        .flush_import_scan(
            library_id,
            ImportFlush {
                thumbnails_to_delete: &result.thumbnails_to_delete,
                books_to_insert: &books_to_insert,
                books_to_update: &books_to_update,
                books_to_link: &books_to_link,
                path_updates: &result.path_updates,
                books_to_delete: &result.books_to_delete,
                sort_keys_dirty,
            },
        )
        .await
    {
        error!(error = %e, library_id, "import flush failed");
    }
}

/// Runs a directory scan and syncs the database for one library.
///
/// When `force` is `false` (incremental mode), files whose stored `mtime` and
/// `file_size` have not changed since the last import are skipped without
/// re-fingerprinting. When `force` is `true` every file is re-fingerprinted
/// regardless of its stored values.
///
/// Sends pinned progress notifications to `hub` via `notif_id` while running.
/// Checks `shutdown` between entries and exits early if shutdown is requested.
/// Dismisses the pinned notification on every return path.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(db, settings, hub, notif_id, shutdown))
)]
#[allow(clippy::too_many_arguments)]
pub async fn run(
    db: &LibraryDb,
    library_id: i64,
    home: &Path,
    install_dir: &Path,
    settings: &ImportSettings,
    force: bool,
    hub: &crate::view::Hub,
    notif_id: ViewId,
    shutdown: &CancellationToken,
) -> ImportOutcome {
    info!(
        library_id,
        home = %home.display(),
        force,
        "import starting"
    );
    let started = Instant::now();
    let ctx = ScanContext {
        hub,
        notif_id,
        shutdown,
    };
    let outcome = {
        let _progress = PinnedProgress::show(hub, notif_id, fl!("importer-importing-library"));
        run_scan(db, library_id, home, install_dir, settings, force, &ctx).await
    };
    if outcome == ImportOutcome::Interrupted {
        hub.send(
            (Event::Notification(NotificationEvent::Show(fl!("importer-import-interrupted"))))
                .into(),
        )
        .ok();
    }
    info!(
        library_id,
        elapsed_ms = started.elapsed().as_millis() as u64,
        outcome = ?outcome,
        "import finished"
    );
    outcome
}

async fn run_scan(
    db: &LibraryDb,
    library_id: i64,
    home: &Path,
    install_dir: &Path,
    settings: &ImportSettings,
    force: bool,
    ctx: &ScanContext<'_>,
) -> ImportOutcome {
    let handles = match db.list_book_handles(library_id).await {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "failed to load book handles for import");
            return ImportOutcome::Failed;
        }
    };

    let mut handles_by_fp: FxHashMap<Fp, (PathBuf, PathBuf)> = handles
        .iter()
        .map(|h| (h.fp, (h.relat.clone(), h.abs.clone())))
        .collect();
    let mut handles_by_path: FxHashMap<PathBuf, Fp> =
        handles.iter().map(|h| (h.relat.clone(), h.fp)).collect();
    let mtime_by_abs: FxHashMap<PathBuf, (UnixTimestamp, FileSize)> = handles
        .iter()
        .filter_map(|h| {
            let mtime = h.mtime?;
            let size = h.file_size?;
            Some((h.abs.clone(), (mtime, size)))
        })
        .collect();
    let mut pending_fps: FxHashSet<Fp> = handles
        .iter()
        .filter(|h| h.status == BookStatus::PendingDiscovery)
        .map(|h| h.fp)
        .collect();

    let purged_fps = db
        .purge_disallowed_books_and_thumbnails(library_id, &settings.allowed_kinds)
        .await
        .unwrap_or_else(|e| {
            error!(error = %e, "failed to purge disallowed books");
            Vec::new()
        });

    for fp in &purged_fps {
        pending_fps.remove(fp);
        if let Some((relat, _abs)) = handles_by_fp.remove(fp) {
            handles_by_path.remove(&relat);
        }
    }

    let home_buf = home.to_path_buf();
    let entries = match tokio::task::spawn_blocking(move || walk_files(&home_buf)).await {
        Ok(entries) => entries,
        Err(error) => {
            error!(error = %error, "library walk failed");
            return ImportOutcome::Failed;
        }
    };

    let mut tracker = ProgressTracker::new();

    let book_statuses = db.all_book_statuses().await.unwrap_or_default();

    let Some(mut result) = scan_entries(
        home,
        install_dir,
        &entries,
        settings,
        force,
        &ctx,
        &mut tracker,
        &mtime_by_abs,
        &mut handles_by_fp,
        &mut handles_by_path,
        &pending_fps,
        &book_statuses,
    )
    .await
    else {
        return ImportOutcome::Interrupted;
    };

    let mut deleted = find_deleted_books(&handles_by_fp, home);
    result.books_to_delete.append(&mut deleted);

    if !result.pending_relocations.is_empty() {
        resolve_relocations(
            db,
            library_id,
            home,
            install_dir,
            settings,
            std::mem::take(&mut result.pending_relocations),
            &mut result,
            &book_statuses,
        )
        .await;
    }

    flush_to_db(db, library_id, result, &purged_fps).await;
    ImportOutcome::Completed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::document::file_extension::FileExtension;
    use crate::library::Library;
    use crate::metadata::{FileInfo, Info};
    use crate::settings::ImportSettings;
    use crate::view::{HubReceiverExt, ViewId};
    use tokio_util::sync::CancellationToken;

    async fn create_migrated_db() -> Database {
        let mut db = Database::new(":memory:").await.expect("in-memory db");
        db.init_for_test(0).await.expect("migrations");
        db
    }

    async fn run_import(dir: &Path, db: &Database, shutdown: &CancellationToken) -> Vec<Event> {
        let lib = Library::new(dir, db, "test")
            .await
            .expect("failed to create library");
        let (tx, mut rx) = crate::view::hub_channel();
        let notif_id = ViewId::MessageNotif(0);
        run(
            &lib.db,
            lib.library_id,
            dir,
            Path::new(""),
            &ImportSettings::default(),
            false,
            &tx,
            notif_id,
            shutdown,
        )
        .await;
        drop(tx);
        rx.try_iter().map(|message| message.event).collect()
    }

    #[tokio::test]
    async fn imports_files_when_not_shutdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db().await;
        std::fs::write(dir.path().join("book.epub"), b"epub content").expect("write");

        let shutdown = CancellationToken::new();
        let events = run_import(dir.path(), &db, &shutdown).await;

        assert!(
            events.iter().any(|e| matches!(e, Event::Close(_))),
            "expected Close event on normal completion"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                Event::Notification(crate::view::NotificationEvent::UpdateProgress(_, 0))
            )),
            "progress should advance past 0"
        );
    }

    #[tokio::test]
    async fn stops_early_when_shutdown_requested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db().await;

        for i in 0..20 {
            std::fs::write(dir.path().join(format!("book{i}.epub")), b"epub content")
                .expect("write");
        }

        let shutdown = CancellationToken::new();
        shutdown.cancel();

        let lib = Library::new(dir.path(), &db, "test")
            .await
            .expect("library");
        let (tx, mut rx) = crate::view::hub_channel();
        let notif_id = ViewId::MessageNotif(0);
        let outcome = run(
            &lib.db,
            lib.library_id,
            dir.path(),
            Path::new(""),
            &ImportSettings::default(),
            false,
            &tx,
            notif_id,
            &shutdown,
        )
        .await;
        drop(tx);
        let events: Vec<Event> = rx.try_iter().map(|message| message.event).collect();

        assert_eq!(outcome, ImportOutcome::Interrupted);
        assert!(
            events.iter().any(|e| matches!(e, Event::Close(_))),
            "notif must be closed even on early exit"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                Event::Notification(crate::view::NotificationEvent::Show(msg))
                    if msg == &fl!("importer-import-interrupted")
            )),
            "interrupted import should tell the user"
        );

        let progress_events: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::Notification(crate::view::NotificationEvent::UpdateProgress(_, _))
                )
            })
            .collect();
        assert!(
            progress_events.len() < 20,
            "shutdown should have cut the scan short (got {} progress events)",
            progress_events.len()
        );
    }

    #[test]
    fn progress_sends_at_100_percent_immediately() {
        let mut tracker = ProgressTracker::new();
        let base = Instant::now();

        let sent = (0..100)
            .filter_map(|i| tracker.should_send(i, 100, base))
            .collect::<Vec<_>>();
        assert_eq!(
            sent,
            vec![1, 100],
            "Only beginning and end when loop is fast"
        );

        assert_eq!(tracker.should_send(99, 100, base), Some(100));
    }

    #[test]
    fn progress_throttled_within_two_seconds() {
        let mut tracker = ProgressTracker::new();
        let base = Instant::now();

        assert_eq!(tracker.should_send(0, 200, base), Some(0));

        assert_eq!(
            tracker.should_send(50, 200, base + Duration::from_millis(500)),
            None
        );
        assert_eq!(
            tracker.should_send(100, 200, base + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn progress_sends_after_two_second_gap() {
        let mut tracker = ProgressTracker::new();
        let base = Instant::now();

        assert_eq!(tracker.should_send(0, 200, base), Some(0));

        assert_eq!(
            tracker.should_send(50, 200, base + Duration::from_secs(2)),
            Some(25)
        );

        assert_eq!(
            tracker.should_send(75, 200, base + Duration::from_secs(3)),
            None
        );

        assert_eq!(
            tracker.should_send(150, 200, base + Duration::from_secs(5)),
            Some(75)
        );
    }

    #[test]
    fn sort_keys_are_dirty_when_purged_or_book_batches_change() {
        assert!(!sort_keys_are_dirty(&[], &ScanResult::empty()));
        assert!(sort_keys_are_dirty(
            &[Fp::from_u64(1)],
            &ScanResult::empty()
        ));

        let mut insert_only = ScanResult::empty();
        insert_only.books_to_insert.push(BookWrite {
            fp: Fp::from_u64(2),
            info: Info::default(),
        });
        assert!(sort_keys_are_dirty(&[], &insert_only));
    }

    #[tokio::test]
    async fn finds_deleted_books_when_file_path_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db().await;
        let lib = Library::new(dir.path(), &db, "test")
            .await
            .expect("library");
        let fp = Fp::from_u64(1);
        let info = Info {
            title: "test".to_string(),
            file: FileInfo {
                path: PathBuf::new(),
                absolute_path: dir.path().join("missing.epub"),
                kind: Some(FileExtension::Epub),
                size: 1,
                mtime: None,
            },
            ..Default::default()
        };

        lib.db
            .batch_insert_books(lib.library_id, &[(fp, &info)])
            .await
            .expect("insert library book");

        let handles = lib
            .db
            .list_book_handles(lib.library_id)
            .await
            .expect("handles");
        let handles_by_fp: FxHashMap<Fp, (PathBuf, PathBuf)> = handles
            .into_iter()
            .map(|h| (h.fp, (h.relat, h.abs)))
            .collect();

        assert_eq!(find_deleted_books(&handles_by_fp, dir.path()), vec![fp]);
    }

    #[tokio::test]
    async fn skips_fingerprinting_disallowed_new_files() {
        use crate::document::file_extension::FileExtension;
        use rustc_hash::FxHashSet;

        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db().await;

        std::fs::write(dir.path().join("book.epub"), b"epub content").expect("write epub");
        std::fs::write(dir.path().join("ignore.xyz"), b"unsupported content").expect("write xyz");

        let mut allowed: FxHashSet<FileExtension> = FxHashSet::default();
        allowed.insert(FileExtension::Epub);
        let settings = ImportSettings {
            allowed_kinds: allowed,
            ..ImportSettings::default()
        };

        let lib = Library::new(dir.path(), &db, "test")
            .await
            .expect("library");
        let (tx, mut rx) = crate::view::hub_channel();
        let notif_id = ViewId::MessageNotif(0);
        let shutdown = CancellationToken::new();

        run(
            &lib.db,
            lib.library_id,
            dir.path(),
            Path::new(""),
            &settings,
            false,
            &tx,
            notif_id,
            &shutdown,
        )
        .await;
        drop(tx);
        let _events: Vec<Event> = rx.try_iter().map(|message| message.event).collect();

        let handles = lib
            .db
            .list_book_handles(lib.library_id)
            .await
            .expect("handles");
        let paths: Vec<_> = handles.iter().map(|h| h.relat.clone()).collect();

        assert!(
            paths.iter().any(|p| p.ends_with("book.epub")),
            "epub should be imported"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with("ignore.xyz")),
            "unsupported kind should not be imported"
        );
    }

    #[tokio::test]
    async fn purges_disallowed_books_on_import() {
        use crate::document::file_extension::FileExtension;
        use rustc_hash::FxHashSet;

        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db().await;

        std::fs::write(dir.path().join("book.epub"), b"epub content").expect("write epub");
        std::fs::write(dir.path().join("doc.pdf"), b"pdf content").expect("write pdf");

        let lib = Library::new(dir.path(), &db, "test")
            .await
            .expect("library");
        let (tx, mut rx) = crate::view::hub_channel();
        let notif_id = ViewId::MessageNotif(0);
        let shutdown = CancellationToken::new();

        run(
            &lib.db,
            lib.library_id,
            dir.path(),
            Path::new(""),
            &ImportSettings::default(),
            false,
            &tx,
            notif_id,
            &shutdown,
        )
        .await;
        drop(tx);
        let _: Vec<Event> = rx.try_iter().map(|message| message.event).collect();

        let handles = lib
            .db
            .list_book_handles(lib.library_id)
            .await
            .expect("handles");
        assert_eq!(handles.len(), 2, "both files should be imported initially");

        let mut epub_only: FxHashSet<FileExtension> = FxHashSet::default();
        epub_only.insert(FileExtension::Epub);

        let settings = ImportSettings {
            allowed_kinds: epub_only,
            ..ImportSettings::default()
        };

        let (tx2, mut rx2) = crate::view::hub_channel();
        run(
            &lib.db,
            lib.library_id,
            dir.path(),
            Path::new(""),
            &settings,
            false,
            &tx2,
            notif_id,
            &shutdown,
        )
        .await;
        drop(tx2);
        let _: Vec<Event> = rx2.try_iter().map(|message| message.event).collect();

        let handles = lib
            .db
            .list_book_handles(lib.library_id)
            .await
            .expect("handles after purge");
        let paths: Vec<_> = handles.iter().map(|h| h.relat.clone()).collect();

        assert_eq!(handles.len(), 1, "only epub should remain after purge");
        assert!(
            paths.iter().any(|p| p.ends_with("book.epub")),
            "epub should still be present"
        );
    }

    #[tokio::test]
    async fn pending_discovery_fills_and_promotes_on_import() {
        use crate::document::file_extension::FileExtension;
        use crate::helpers::Fingerprint;
        use crate::library::book_status::BookStatus;
        use rustc_hash::FxHashSet;

        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db().await;
        let book_path = dir.path().join("pending.epub");
        std::fs::write(&book_path, b"pending discovery content").expect("write epub");
        let fp = book_path.fingerprint().expect("fingerprint");
        let fp_str = fp.to_string();
        let file_meta = std::fs::metadata(&book_path).expect("metadata");
        let file_size = file_meta.len() as i64;
        let file_mtime = file_meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| UnixTimestamp::from((d.as_secs().div_ceil(2) * 2) as i64))
            .expect("mtime");
        let now = UnixTimestamp::now();

        let lib = Library::new(dir.path(), &db, "test")
            .await
            .expect("library");

        async {
            sqlx::query!(
                r#"
                INSERT INTO books (fingerprint, file_kind, file_size, added_at, status)
                VALUES (?, '', 0, ?, ?)
                "#,
                fp_str,
                now,
                BookStatus::PendingDiscovery,
            )
            .execute(db.pool())
            .await
            .expect("insert stub");

            let abs = book_path.to_string_lossy().into_owned();
            sqlx::query!(
                r#"
                INSERT INTO library_books (
                    library_id, book_fingerprint, added_to_library_at,
                    file_path, absolute_path, mtime, file_size
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
                lib.library_id,
                fp_str,
                now,
                "pending.epub",
                abs,
                file_mtime,
                file_size,
            )
            .execute(db.pool())
            .await
            .expect("link stub with matching mtime path");
        }
        .await;

        let mut allowed: FxHashSet<FileExtension> = FxHashSet::default();
        allowed.insert(FileExtension::Epub);
        let settings = ImportSettings {
            allowed_kinds: allowed,
            ..ImportSettings::default()
        };

        let (tx, mut rx) = crate::view::hub_channel();
        let notif_id = ViewId::MessageNotif(0);
        let shutdown = CancellationToken::new();

        run(
            &lib.db,
            lib.library_id,
            dir.path(),
            Path::new(""),
            &settings,
            false,
            &tx,
            notif_id,
            &shutdown,
        )
        .await;
        drop(tx);
        let _: Vec<Event> = rx.try_iter().map(|message| message.event).collect();

        let handles = lib
            .db
            .list_book_handles(lib.library_id)
            .await
            .expect("handles");
        let handle = handles
            .iter()
            .find(|h| h.fp == fp)
            .expect("pending book handle");
        assert_eq!(handle.status, BookStatus::Active);

        let books = lib.db.get_all_books(lib.library_id).await.expect("shelf");
        assert!(
            books
                .iter()
                .any(|b| b.file.kind == Some(FileExtension::Epub)),
            "filled book should appear on shelf with kind"
        );
    }
}
