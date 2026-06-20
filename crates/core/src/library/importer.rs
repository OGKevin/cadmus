use crate::db::types::{FileSize, UnixTimestamp};
use crate::document::file_kind;
use crate::fl;
use crate::helpers::{Fingerprint, Fp, IsHidden};
use crate::library::db::{Db as LibraryDb, PathUpdate};
use crate::metadata::{FileInfo, Info, extract_metadata_from_document};
use crate::settings::ImportSettings;
use crate::task::ShutdownSignal;
use crate::view::{Event, NotificationEvent, ViewId};
use fxhash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, UNIX_EPOCH};
use tracing::{debug, error, info};
use walkdir::{DirEntry, WalkDir};

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
    hub: &'a Sender<Event>,
    notif_id: ViewId,
    shutdown: &'a ShutdownSignal,
}

struct ScanResult {
    books_to_insert: Vec<(Fp, Info)>,
    path_updates: Vec<PathUpdate>,
    books_to_delete: Vec<Fp>,
    pending_relocations: Vec<PendingRelocation>,
    thumbnails_to_delete: Vec<Fp>,
}

#[cfg(feature = "emulator")]
const IGNORED_TOP_LEVEL_DIRS: &[&str] = &["target", "node_modules", "thirdparty"];

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
            settings,
            ctx,
            tracker,
            mtime_by_abs,
            handles_by_fp,
            handles_by_path
        ),
        fields(total)
    )
)]
fn scan_entries(
    home: &Path,
    entries: &[DirEntry],
    settings: &ImportSettings,
    force: bool,
    ctx: &ScanContext<'_>,
    tracker: &mut ProgressTracker,
    mtime_by_abs: &FxHashMap<PathBuf, (UnixTimestamp, FileSize)>,
    handles_by_fp: &mut FxHashMap<Fp, (PathBuf, PathBuf)>,
    handles_by_path: &mut FxHashMap<PathBuf, Fp>,
) -> Option<ScanResult> {
    let total = entries.len();
    tracing::Span::current().record("total", total);
    let mut skipped_count = 0u32;
    let mut fingerprinted_count = 0u32;
    let mut mtime_miss_count = 0u32;
    debug!(mtime_map_size = mtime_by_abs.len(), "starting scan");

    let mut books_to_insert: Vec<(Fp, Info)> = Vec::new();
    let mut path_updates: Vec<PathUpdate> = Vec::new();
    let mut books_to_delete: Vec<Fp> = Vec::new();
    let mut pending_relocations: Vec<PendingRelocation> = Vec::new();
    let mut thumbnails_to_delete: Vec<Fp> = Vec::new();

    for (idx, entry) in entries.iter().enumerate() {
        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!("procssing entry", entry = ?entry).entered();

        if ctx.shutdown.should_stop() {
            tracing::info!("import scan interrupted by shutdown");
            return None;
        }

        let path = entry.path();
        let relat = path.strip_prefix(home).unwrap_or(path);

        let kind = file_kind(path);
        let is_known_to_db = handles_by_path.contains_key(relat);
        let allowed_kind = kind.filter(|k| settings.is_kind_allowed(*k));

        if !is_known_to_db && allowed_kind.is_none() {
            send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
            continue;
        }

        let file_meta = match entry.metadata() {
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

        if !force && let Some(mtime) = current_mtime {
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

        let fp = match path.fingerprint() {
            Ok(fp) => {
                fingerprinted_count += 1;
                fp
            }
            Err(e) => {
                error!(path = ?path, error = %e, "failed to compute fingerprint, skipping");
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

            path_updates.push(PathUpdate {
                fp,
                relat: relat.to_path_buf(),
                abs: path.to_path_buf(),
                mtime: current_mtime,
                file_size: Some(current_size),
            });

            send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
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
            books_to_delete.push(old_fp);

            pending_relocations.push(PendingRelocation::FingerprintChanged {
                new_fp: fp,
                old_fp,
                file_size: i64::from(current_size) as u64,
            });

            thumbnails_to_delete.push(old_fp);
            path_updates.push(PathUpdate {
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
                    kind: kind.as_str().to_owned(),
                    size,
                    mtime: current_mtime,
                },
                ..Default::default()
            };
            if settings.metadata_kinds.contains(&book_info.file.kind) {
                extract_metadata_from_document(home, &mut book_info);
            }
            handles_by_fp.insert(fp, (relat.to_path_buf(), path.to_path_buf()));
            handles_by_path.insert(relat.to_path_buf(), fp);
            books_to_insert.push((fp, book_info));
        }

        send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
    }

    info!(
        total,
        skipped = skipped_count,
        fingerprinted = fingerprinted_count,
        mtime_misses = mtime_miss_count,
        "scan complete"
    );

    Some(ScanResult {
        books_to_insert,
        path_updates,
        books_to_delete,
        pending_relocations,
        thumbnails_to_delete,
    })
}

fn send_progress(
    hub: &Sender<Event>,
    notif_id: ViewId,
    tracker: &mut ProgressTracker,
    idx: usize,
    total: usize,
) {
    let Some(percent) = tracker.should_send(idx, total, Instant::now()) else {
        return;
    };
    debug!(percent, "import progress");
    hub.send(Event::Notification(NotificationEvent::UpdateProgress(
        notif_id, percent,
    )))
    .ok();
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(db, home, settings, pending_relocations, books_to_insert))
)]
fn resolve_relocations(
    db: &LibraryDb,
    library_id: i64,
    home: &Path,
    settings: &ImportSettings,
    pending_relocations: Vec<PendingRelocation>,
    books_to_insert: &mut Vec<(Fp, Info)>,
) {
    let old_fps: Vec<Fp> = pending_relocations
        .iter()
        .map(PendingRelocation::old_fp)
        .collect();

    let mut fetched = db
        .batch_get_books_by_fingerprints(library_id, &old_fps)
        .unwrap_or_default();

    for relocation in pending_relocations {
        match relocation {
            PendingRelocation::FingerprintChanged {
                new_fp,
                old_fp,
                file_size,
            } => {
                if let Some(mut info) = fetched.remove(&old_fp) {
                    if settings.sync_metadata && settings.metadata_kinds.contains(&info.file.kind) {
                        extract_metadata_from_document(home, &mut info);
                    }
                    info.file.size = file_size;
                    books_to_insert.push((new_fp, info));
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

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(
        db,
        books_to_insert,
        path_updates,
        books_to_delete,
        thumbnails_to_delete
    ))
)]
fn flush_to_db(
    db: &LibraryDb,
    library_id: i64,
    books_to_insert: Vec<(Fp, Info)>,
    path_updates: Vec<PathUpdate>,
    books_to_delete: Vec<Fp>,
    thumbnails_to_delete: Vec<Fp>,
) {
    if let Err(e) = db.batch_delete_thumbnails(&thumbnails_to_delete) {
        error!(
            error = %e,
            count = thumbnails_to_delete.len(),
            "batch delete thumbnails failed"
        );
    }

    if !books_to_insert.is_empty() {
        let book_refs: Vec<(Fp, &Info)> = books_to_insert
            .iter()
            .map(|(fp, info)| (*fp, info))
            .collect();
        if let Err(e) = db.batch_insert_books(library_id, &book_refs) {
            error!(error = %e, count = book_refs.len(), "batch insert failed");
        }
    }

    if let Err(e) = db.batch_update_book_paths(library_id, &path_updates) {
        error!(
            error = %e,
            count = path_updates.len(),
            "batch update book paths failed"
        );
    }

    if !books_to_delete.is_empty() {
        if let Err(e) = db.batch_delete_books(library_id, &books_to_delete) {
            error!(error = %e, count = books_to_delete.len(), "batch delete failed");
        }
    }

    if let Err(e) = db.compute_sort_keys(library_id) {
        error!(error = %e, library_id, "failed to compute sort keys");
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
/// On completion or early exit, closes the notification and returns.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(db, settings, hub, notif_id, shutdown))
)]
pub fn run(
    db: &LibraryDb,
    library_id: i64,
    home: &Path,
    settings: &ImportSettings,
    force: bool,
    hub: &Sender<Event>,
    notif_id: ViewId,
    shutdown: &ShutdownSignal,
) {
    hub.send(Event::Notification(NotificationEvent::ShowPinned(
        notif_id,
        fl!("importer-importing-library"),
    )))
    .ok();

    let handles = match db.list_book_handles(library_id) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "failed to load book handles for import");
            hub.send(Event::Close(notif_id)).ok();
            return;
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

    let purged_fps = db
        .delete_books_with_disallowed_kinds(library_id, &settings.allowed_kinds)
        .unwrap_or_else(|e| {
            error!(error = %e, "failed to purge disallowed books");
            Vec::new()
        });

    for fp in &purged_fps {
        if let Some((relat, _abs)) = handles_by_fp.remove(fp) {
            handles_by_path.remove(&relat);
        }
    }

    if !purged_fps.is_empty() {
        if let Err(e) = db.batch_delete_thumbnails(&purged_fps) {
            error!(error = %e, count = purged_fps.len(), "failed to delete thumbnails for purged books");
        }
    }

    let entries = walk_files(home);

    let ctx = ScanContext {
        hub,
        notif_id,
        shutdown,
    };

    let mut tracker = ProgressTracker::new();

    let Some(mut result) = scan_entries(
        home,
        &entries,
        settings,
        force,
        &ctx,
        &mut tracker,
        &mtime_by_abs,
        &mut handles_by_fp,
        &mut handles_by_path,
    ) else {
        hub.send(Event::Close(notif_id)).ok();
        return;
    };

    let mut deleted = find_deleted_books(&handles_by_fp, home);
    result.books_to_delete.append(&mut deleted);

    if !result.pending_relocations.is_empty() {
        resolve_relocations(
            db,
            library_id,
            home,
            settings,
            result.pending_relocations,
            &mut result.books_to_insert,
        );
    }

    flush_to_db(
        db,
        library_id,
        result.books_to_insert,
        result.path_updates,
        result.books_to_delete,
        result.thumbnails_to_delete,
    );

    hub.send(Event::Close(notif_id)).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::library::Library;
    use crate::metadata::{FileInfo, Info};
    use crate::settings::ImportSettings;
    use crate::task::ShutdownSignal;
    use crate::view::ViewId;
    use std::sync::mpsc;

    fn create_migrated_db() -> Database {
        let mut db = Database::new(":memory:").expect("in-memory db");
        db.init(0).expect("migrations");
        db
    }

    fn run_import(dir: &Path, db: &Database, shutdown: &ShutdownSignal) -> Vec<Event> {
        let lib = Library::new(dir, db, "test").expect("failed to create library");
        let (tx, rx) = mpsc::channel();
        let notif_id = ViewId::MessageNotif(0);
        run(
            &lib.db,
            lib.library_id,
            dir,
            &ImportSettings::default(),
            false,
            &tx,
            notif_id,
            shutdown,
        );
        drop(tx);
        rx.try_iter().collect()
    }

    #[test]
    fn imports_files_when_not_shutdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db();
        std::fs::write(dir.path().join("book.epub"), b"epub content").expect("write");

        let shutdown = ShutdownSignal::never();
        let events = run_import(dir.path(), &db, &shutdown);

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

    #[test]
    fn stops_early_when_shutdown_requested() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db();

        for i in 0..20 {
            std::fs::write(dir.path().join(format!("book{i}.epub")), b"epub content")
                .expect("write");
        }

        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let shutdown = ShutdownSignal::new_for_test(shutdown_rx);

        // Signal shutdown before the import starts so scan_entries exits immediately.
        shutdown_tx.send(()).expect("send shutdown");

        let lib = Library::new(dir.path(), &db, "test").expect("library");
        let (tx, rx) = mpsc::channel();
        let notif_id = ViewId::MessageNotif(0);
        run(
            &lib.db,
            lib.library_id,
            dir.path(),
            &ImportSettings::default(),
            false,
            &tx,
            notif_id,
            &shutdown,
        );
        drop(tx);
        let events: Vec<Event> = rx.try_iter().collect();

        assert!(
            events.iter().any(|e| matches!(e, Event::Close(_))),
            "notif must be closed even on early exit"
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
    fn finds_deleted_books_when_file_path_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db();
        let lib = Library::new(dir.path(), &db, "test").expect("library");
        let fp = Fp::from_u64(1);
        let info = Info {
            title: "test".to_string(),
            file: FileInfo {
                path: PathBuf::new(),
                absolute_path: dir.path().join("missing.epub"),
                kind: "epub".to_string(),
                size: 1,
                mtime: None,
            },
            ..Default::default()
        };

        lib.db
            .batch_insert_books(lib.library_id, &[(fp, &info)])
            .expect("insert library book");

        let handles = lib.db.list_book_handles(lib.library_id).expect("handles");
        let handles_by_fp: FxHashMap<Fp, (PathBuf, PathBuf)> = handles
            .into_iter()
            .map(|h| (h.fp, (h.relat, h.abs)))
            .collect();

        assert_eq!(find_deleted_books(&handles_by_fp, dir.path()), vec![fp]);
    }

    #[test]
    fn skips_fingerprinting_disallowed_new_files() {
        use crate::settings::FileExtension;
        use fxhash::FxHashSet;

        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db();

        std::fs::write(dir.path().join("book.epub"), b"epub content").expect("write epub");
        std::fs::write(dir.path().join("ignore.xyz"), b"unsupported content").expect("write xyz");

        let mut allowed: FxHashSet<FileExtension> = FxHashSet::default();
        allowed.insert(FileExtension::Epub);
        let settings = ImportSettings {
            allowed_kinds: allowed,
            ..ImportSettings::default()
        };

        let lib = Library::new(dir.path(), &db, "test").expect("library");
        let (tx, rx) = std::sync::mpsc::channel();
        let notif_id = ViewId::MessageNotif(0);
        let shutdown = ShutdownSignal::never();

        run(
            &lib.db,
            lib.library_id,
            dir.path(),
            &settings,
            false,
            &tx,
            notif_id,
            &shutdown,
        );
        drop(tx);
        let _events: Vec<Event> = rx.try_iter().collect();

        let handles = lib.db.list_book_handles(lib.library_id).expect("handles");
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

    #[test]
    fn purges_disallowed_books_on_import() {
        use crate::settings::FileExtension;
        use fxhash::FxHashSet;

        let dir = tempfile::tempdir().expect("tempdir");
        let db = create_migrated_db();

        std::fs::write(dir.path().join("book.epub"), b"epub content").expect("write epub");
        std::fs::write(dir.path().join("doc.pdf"), b"pdf content").expect("write pdf");

        let lib = Library::new(dir.path(), &db, "test").expect("library");
        let (tx, rx) = std::sync::mpsc::channel();
        let notif_id = ViewId::MessageNotif(0);
        let shutdown = ShutdownSignal::never();

        run(
            &lib.db,
            lib.library_id,
            dir.path(),
            &ImportSettings::default(),
            false,
            &tx,
            notif_id,
            &shutdown,
        );
        drop(tx);
        let _: Vec<Event> = rx.try_iter().collect();

        let handles = lib.db.list_book_handles(lib.library_id).expect("handles");
        assert_eq!(handles.len(), 2, "both files should be imported initially");

        let mut epub_only: FxHashSet<FileExtension> = FxHashSet::default();
        epub_only.insert(FileExtension::Epub);

        let settings = ImportSettings {
            allowed_kinds: epub_only,
            ..ImportSettings::default()
        };

        let (tx2, rx2) = std::sync::mpsc::channel();
        run(
            &lib.db,
            lib.library_id,
            dir.path(),
            &settings,
            false,
            &tx2,
            notif_id,
            &shutdown,
        );
        drop(tx2);
        let _: Vec<Event> = rx2.try_iter().collect();

        let handles = lib
            .db
            .list_book_handles(lib.library_id)
            .expect("handles after purge");
        let paths: Vec<_> = handles.iter().map(|h| h.relat.clone()).collect();

        assert_eq!(handles.len(), 1, "only epub should remain after purge");
        assert!(
            paths.iter().any(|p| p.ends_with("book.epub")),
            "epub should still be present"
        );
    }
}
