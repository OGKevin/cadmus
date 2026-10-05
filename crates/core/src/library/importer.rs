#[cfg(test)]
use crate::db::types::UnixTimestamp;
use crate::document::file_kind;
use crate::fl;
use crate::helpers::{Fingerprint, FingerprintStamp, Fp, IsHidden};
use crate::library::book_status::BookStatus;
use crate::library::db::{Db as LibraryDb, ImportFlush, PathUpdate};
use crate::metadata::{FileInfo, Info, extract_metadata_from_document};
use crate::settings::ImportSettings;
use crate::view::notification::PinnedProgress;
use crate::view::{Event, NotificationEvent, ViewId};
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
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

/// A book whose fingerprint changed, so its record has to be rebuilt under the
/// new fingerprint while the old one is dropped.
struct PendingRelocation {
    new_fp: Fp,
    old_fp: Fp,
    file_size: u64,
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

        if !self.first_tick && percent == self.last_percent {
            return None;
        }

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
/// The document handle stays inside the blocking closure so it never crosses an
/// await. Returns `None` if the task panicked, in which case the caller keeps
/// the file-level fields it already had; only the document-derived fields are
/// lost, which a panic would have left untrustworthy anyway.
///
/// Callers keep a `FileInfo` clone to restore that fallback rather than an
/// `Info` clone: the former is the only part they already knew, while the
/// latter copied every title, category and path for every new book.
async fn extract_metadata(home: &Path, install_dir: &Path, info: Info) -> Option<Info> {
    let home = home.to_path_buf();
    let install_dir = install_dir.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let mut info = info;
        extract_metadata_from_document(&home, &mut info, &install_dir);
        info
    })
    .await
    {
        Ok(info) => Some(info),
        Err(error) => {
            error!(error = %error, "metadata extraction task panicked");
            None
        }
    }
}

/// A walked file with the stamp its directory entry already exposed.
///
/// Capturing the stamp during the blocking walk keeps the async scan from
/// calling `std::fs::metadata` on a runtime worker, and `DirEntry::metadata`
/// does not follow symlinks the way `Path::stamp` would.
struct ScannedEntry {
    entry: DirEntry,
    stamp: Option<FingerprintStamp>,
}

/// Collects every scannable file under `home`, plus the roots the walk could not
/// read.
///
/// A book missing from `files` is not necessarily deleted: it may live under a
/// directory the walk failed on. Deleting on that basis loses the book and its
/// reading state, so those roots are reported and the caller keeps every book
/// beneath them. Hidden and ignored entries are skipped without being reported,
/// so the caller re-checks them with `try_exists` instead of keeping them
/// unconditionally.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(home)))]
fn walk_files(home: &Path) -> (Vec<ScannedEntry>, FxHashSet<PathBuf>) {
    let mut unreadable = FxHashSet::default();
    let mut files = Vec::new();
    let mut walker = WalkDir::new(home).min_depth(1).into_iter();

    let mut record = |path: &Path| {
        unreadable.insert(path.strip_prefix(home).unwrap_or(path).to_path_buf());
    };

    while let Some(entry) = walker.next() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                if let Some(path) = err.path() {
                    record(path);
                }
                continue;
            }
        };

        if entry.is_hidden() || is_ignored_dir(&entry) {
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }

        if !entry.file_type().is_dir() {
            let stamp = entry
                .metadata()
                .ok()
                .map(|meta| FingerprintStamp::from_metadata(&meta));
            files.push(ScannedEntry { entry, stamp });
        }
    }

    (files, unreadable)
}

#[cfg(feature = "emulator")]
fn is_ignored_dir(entry: &DirEntry) -> bool {
    entry.depth() == 1
        && entry.file_type().is_dir()
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| IGNORED_TOP_LEVEL_DIRS.contains(&name))
}

#[cfg(not(feature = "emulator"))]
fn is_ignored_dir(_entry: &DirEntry) -> bool {
    false
}

enum ScanIterResult {
    Continue,
    Abort,
}

/// Mutable scan state that outlives a single entry.
///
/// Borrowed for the whole scan, so passing it as one `&mut` lets
/// [`process_scan_entry`] mutate the handles, the result and the counters
/// together instead of threading six separate `&mut` borrows.
struct ScanState<'a> {
    handles_by_fp: &'a mut FxHashMap<Fp, (PathBuf, PathBuf)>,
    handles_by_path: &'a mut FxHashMap<PathBuf, Fp>,
    result: &'a mut ScanResult,
    skipped_count: &'a mut u32,
    fingerprinted_count: &'a mut u32,
    mtime_miss_count: &'a mut u32,
}

/// Read-only inputs for one scanned entry.
///
/// Grouped so [`process_scan_entry`] takes a value it can destructure and a
/// single mutable state, rather than thirteen positional arguments.
struct ScanInputs<'a> {
    home: &'a Path,
    install_dir: &'a Path,
    entry: &'a DirEntry,
    stamp: Option<FingerprintStamp>,
    idx: usize,
    total: usize,
    settings: &'a ImportSettings,
    force: bool,
    ctx: &'a ScanContext<'a>,
    mtime_by_abs: &'a FxHashMap<PathBuf, FingerprintStamp>,
    pending_fps: &'a FxHashSet<Fp>,
    book_statuses: &'a FxHashMap<Fp, BookStatus>,
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip_all, fields(entry = ?inputs.entry), level = tracing::Level::TRACE)
)]
async fn process_scan_entry<'a>(
    inputs: ScanInputs<'a>,
    tracker: &mut ProgressTracker,
    state: &mut ScanState<'_>,
) -> ScanIterResult {
    let ScanInputs {
        home,
        install_dir,
        entry,
        stamp,
        idx,
        total,
        settings,
        force,
        ctx,
        mtime_by_abs,
        pending_fps,
        book_statuses,
    } = inputs;
    let ScanState {
        handles_by_fp,
        handles_by_path,
        result,
        skipped_count,
        fingerprinted_count,
        mtime_miss_count,
    } = &mut *state;

    if ctx.shutdown.is_cancelled() {
        tracing::info!("import scan interrupted by shutdown");
        return ScanIterResult::Abort;
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
        return ScanIterResult::Continue;
    }

    let Some(current) = stamp else {
        error!(path = ?path, "failed to read metadata, skipping");
        send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
        return ScanIterResult::Continue;
    };
    let current_size = current.size;
    let current_mtime = current.mtime;

    if !force && !path_is_pending {
        match mtime_by_abs.get(path) {
            Some(&stored) if current.is_unchanged_from(&stored) => {
                **skipped_count += 1;
                debug!(path = %relat.display(), "mtime and size unchanged, skipping fingerprint");
                send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
                return ScanIterResult::Continue;
            }
            None => {
                **mtime_miss_count += 1;
                debug!(
                    path = %relat.display(),
                    abs = %path.display(),
                    "mtime lookup miss: file not in mtime_by_abs map"
                );
            }
            Some(_) => {}
        }
    }

    let fp = match path.fingerprint().await {
        Ok(fp) => {
            **fingerprinted_count += 1;
            fp
        }
        Err(e) => {
            error!(path = ?path, error = %e, "failed to compute fingerprint, skipping");
            send_progress(ctx.hub, ctx.notif_id, tracker, idx, total);
            return ScanIterResult::Continue;
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
                    let file = book_info.file.clone();
                    book_info = extract_metadata(home, install_dir, book_info)
                        .await
                        .unwrap_or(Info {
                            file,
                            ..Default::default()
                        });
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
        return ScanIterResult::Continue;
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

        result.pending_relocations.push(PendingRelocation {
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
        return ScanIterResult::Continue;
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
            let file = book_info.file.clone();
            book_info = extract_metadata(home, install_dir, book_info)
                .await
                .unwrap_or(Info {
                    file,
                    ..Default::default()
                });
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
    ScanIterResult::Continue
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
    entries: &[ScannedEntry],
    settings: &ImportSettings,
    force: bool,
    ctx: &ScanContext<'_>,
    tracker: &mut ProgressTracker,
    mtime_by_abs: &FxHashMap<PathBuf, FingerprintStamp>,
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

    let mut state = ScanState {
        handles_by_fp,
        handles_by_path,
        result: &mut result,
        skipped_count: &mut skipped_count,
        fingerprinted_count: &mut fingerprinted_count,
        mtime_miss_count: &mut mtime_miss_count,
    };

    for (idx, scanned) in entries.iter().enumerate() {
        let iter_result = process_scan_entry(
            ScanInputs {
                home,
                install_dir,
                entry: &scanned.entry,
                stamp: scanned.stamp,
                idx,
                total,
                settings,
                force,
                ctx,
                mtime_by_abs,
                pending_fps,
                book_statuses,
            },
            tracker,
            &mut state,
        )
        .await;
        match iter_result {
            ScanIterResult::Continue => {}
            ScanIterResult::Abort => return None,
        }
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
        .map(|relocation| relocation.old_fp)
        .collect();

    let mut fetched = match db
        .batch_get_books_by_fingerprints(library_id, &old_fps)
        .await
    {
        Ok(books) => books,
        Err(error) => {
            tracing::error!(error = %error, "skipping relocations after fingerprint lookup failed");
            return;
        }
    };

    for relocation in &pending_relocations {
        if let Some(mut info) = fetched.remove(&relocation.old_fp) {
            if settings.sync_metadata
                && info
                    .file
                    .kind
                    .is_some_and(|k| settings.metadata_kinds.contains(&k))
            {
                let fallback = info.clone();
                info = extract_metadata(home, install_dir, info)
                    .await
                    .unwrap_or(fallback);
            }
            info.file.size = relocation.file_size;
            result.push_new_book(
                book_statuses.get(&relocation.new_fp).copied(),
                BookWrite {
                    fp: relocation.new_fp,
                    info,
                },
            );
        }
    }
}

/// Book paths the walk did not report, split into the ones that are provably
/// gone and the ones that are merely invisible to this scan.
///
/// Absence from the walk set is not proof of deletion. A book under a directory
/// the walk could not read is absent for the same reason a deleted book is, so
/// it is kept unexamined. Every other absent path, including one the walk
/// skipped as hidden, is re-checked with `try_exists`: only an explicit `false`
/// justifies a delete.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(handles_by_fp)))]
async fn find_deleted_books(
    handles_by_fp: &FxHashMap<Fp, (PathBuf, PathBuf)>,
    home: &Path,
    unreadable: &FxHashSet<PathBuf>,
    existing_relat_paths: &FxHashSet<PathBuf>,
) -> Vec<Fp> {
    let mut deleted = Vec::new();
    let mut absent: Vec<(Fp, PathBuf)> = Vec::new();

    for (fp, (relat, _)) in handles_by_fp {
        if !relat.as_os_str().is_empty() && existing_relat_paths.contains(relat) {
            continue;
        }
        if relat.as_os_str().is_empty() {
            info!(fp = %fp, path = %relat.display(), "removing deleted entry");
            deleted.push(*fp);
            continue;
        }
        if unreadable.iter().any(|root| relat.starts_with(root)) {
            warn!(
                fp = %fp,
                path = %relat.display(),
                "keeping book under directory the scan could not read"
            );
            continue;
        }
        absent.push((*fp, home.join(relat)));
    }

    for (fp, abs) in absent {
        match tokio::fs::try_exists(&abs).await {
            Ok(false) => {
                info!(fp = %fp, path = %abs.display(), "removing deleted entry");
                deleted.push(fp);
            }
            Ok(true) => {
                warn!(
                    fp = %fp,
                    path = %abs.display(),
                    "scan did not report an existing book, keeping it"
                );
            }
            Err(e) => {
                warn!(
                    fp = %fp,
                    path = %abs.display(),
                    error = %e,
                    "could not stat book during delete check, keeping it"
                );
            }
        }
    }

    deleted
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
    let mtime_by_abs: FxHashMap<PathBuf, FingerprintStamp> = handles
        .iter()
        .filter_map(|h| {
            Some((
                h.abs.clone(),
                FingerprintStamp {
                    mtime: h.mtime,
                    size: h.file_size?,
                },
            ))
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
    let (entries, unreadable) =
        match tokio::task::spawn_blocking(move || walk_files(&home_buf)).await {
            Ok(walked) => walked,
            Err(error) => {
                error!(error = %error, "library walk failed");
                return ImportOutcome::Failed;
            }
        };

    if !unreadable.is_empty() {
        warn!(
            roots = unreadable.len(),
            "scan could not read part of the library; books under it are kept"
        );
    }

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

    let existing_relat_paths: FxHashSet<PathBuf> = entries
        .iter()
        .filter_map(|scanned| {
            scanned
                .entry
                .path()
                .strip_prefix(home)
                .ok()
                .map(|path| path.to_path_buf())
        })
        .collect();
    let mut deleted =
        find_deleted_books(&handles_by_fp, home, &unreadable, &existing_relat_paths).await;
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

        assert_eq!(
            tracker.should_send(99, 100, base),
            None,
            "a repeated 100% adds nothing to the bar"
        );
    }

    #[test]
    fn progress_skips_an_unchanged_percent_across_throttle_windows() {
        let mut tracker = ProgressTracker::new();
        let base = Instant::now();

        assert_eq!(tracker.should_send(0, 1000, base), Some(0));

        let later = base + Duration::from_secs(30);
        assert_eq!(
            tracker.should_send(5, 1000, later),
            None,
            "the throttle window elapsed but the bar would not move"
        );
        assert_eq!(tracker.should_send(9, 1000, later), Some(1));
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

        let existing = FxHashSet::default();
        let unreadable = FxHashSet::default();
        assert_eq!(
            find_deleted_books(&handles_by_fp, dir.path(), &unreadable, &existing).await,
            vec![fp]
        );
    }

    #[tokio::test]
    async fn import_removes_active_books_with_empty_file_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("on-disk.epub"), b"epub content").expect("write");

        let db = create_migrated_db().await;
        let lib = Library::new(dir.path(), &db, "test")
            .await
            .expect("library");

        let on_disk_fp = dir
            .path()
            .join("on-disk.epub")
            .fingerprint()
            .await
            .expect("fp");
        let on_disk_info = Info {
            title: "On disk".to_string(),
            file: FileInfo {
                path: PathBuf::from("on-disk.epub"),
                absolute_path: dir.path().join("on-disk.epub"),
                kind: Some(FileExtension::Epub),
                size: 12,
                mtime: None,
            },
            ..Default::default()
        };
        lib.db
            .batch_insert_books(lib.library_id, &[(on_disk_fp, &on_disk_info)])
            .await
            .expect("insert on-disk book");

        let ghost_fp = Fp::from_u64(99);
        let ghost_info = Info {
            title: "Ghost".to_string(),
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
            .batch_insert_books(lib.library_id, &[(ghost_fp, &ghost_info)])
            .await
            .expect("insert ghost book");

        let shutdown = CancellationToken::new();
        run_import(dir.path(), &db, &shutdown).await;

        let books = lib.db.get_all_books(lib.library_id).await.expect("shelf");
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].title, "On disk");
        assert!(
            books.iter().all(|b| b.fp != Some(ghost_fp)),
            "empty-path row should be removed by import"
        );
    }

    #[tokio::test]
    async fn keeps_books_the_scan_could_not_read() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let sub = dir.path().join("locked");
        std::fs::create_dir(&sub).expect("create subdir");
        let abs = sub.join("hidden.epub");
        std::fs::write(&abs, b"content").expect("write book");
        let relat = PathBuf::from("locked/hidden.epub");
        let fp = abs.fingerprint().await.expect("fingerprint");
        let info = Info {
            file: FileInfo {
                path: relat.clone(),
                absolute_path: abs.clone(),
                kind: Some(FileExtension::Epub),
                size: 7,
                mtime: None,
            },
            ..Default::default()
        };

        let db = create_migrated_db().await;
        let lib = Library::new(dir.path(), &db, "test")
            .await
            .expect("library");
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

        tokio::fs::remove_file(&abs).await.expect("remove book");
        let unreadable: FxHashSet<PathBuf> = FxHashSet::from_iter([PathBuf::from("locked")]);
        let existing = FxHashSet::default();
        assert!(
            find_deleted_books(&handles_by_fp, dir.path(), &unreadable, &existing)
                .await
                .is_empty(),
            "book under an unreadable root must not be deleted"
        );
    }

    #[tokio::test]
    async fn keeps_book_the_scan_missed_but_disk_still_has() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let abs = dir.path().join("kept.epub");
        std::fs::write(&abs, b"content").expect("write book");
        let relat = PathBuf::from("kept.epub");
        let fp = abs.fingerprint().await.expect("fingerprint");
        let info = Info {
            file: FileInfo {
                path: relat.clone(),
                absolute_path: abs.clone(),
                kind: Some(FileExtension::Epub),
                size: 7,
                mtime: None,
            },
            ..Default::default()
        };

        let db = create_migrated_db().await;
        let lib = Library::new(dir.path(), &db, "test")
            .await
            .expect("library");
        lib.db
            .batch_insert_books(lib.library_id, &[(fp, &info)])
            .await
            .expect("insert library book");

        let handles_by_fp: FxHashMap<Fp, (PathBuf, PathBuf)> = lib
            .db
            .list_book_handles(lib.library_id)
            .await
            .expect("handles")
            .into_iter()
            .map(|h| (h.fp, (h.relat, h.abs)))
            .collect();

        let existing = FxHashSet::default();
        let unreadable = FxHashSet::default();
        assert!(
            find_deleted_books(&handles_by_fp, dir.path(), &unreadable, &existing)
                .await
                .is_empty(),
            "an existing file must not be deleted just because the walk missed it"
        );
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
        let fp = book_path.fingerprint().await.expect("fingerprint");
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

    #[test]
    fn walk_files_skips_hidden_entries_without_reporting_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".hidden")).expect("hidden dir");
        std::fs::write(dir.path().join(".hidden/book.epub"), b"x").expect("hidden book");
        std::fs::write(dir.path().join("visible.epub"), b"x").expect("visible book");

        let (files, unreadable) = walk_files(dir.path());

        let visible = files
            .iter()
            .find(|scanned| scanned.entry.path() == dir.path().join("visible.epub"))
            .expect("visible book must be walked");
        assert!(
            visible.stamp.is_some(),
            "the walk must capture the entry stamp"
        );
        assert!(
            !files
                .iter()
                .any(|scanned| scanned.entry.path().ends_with(".hidden/book.epub")),
            "hidden entries must not be walked"
        );
        assert!(
            unreadable.is_empty(),
            "hidden entries must not be reported as unreadable"
        );
    }
}
