use crate::document::file_kind;
use crate::fl;
use crate::helpers::{Fingerprint, Fp, IsHidden};
use crate::library::db::Db as LibraryDb;
use crate::metadata::{extract_metadata_from_document, FileInfo, Info};
use crate::settings::ImportSettings;
use crate::task::ShutdownSignal;
use crate::view::{Event, NotificationEvent, ViewId};
use fxhash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
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

struct ScanContext<'a> {
    hub: &'a Sender<Event>,
    notif_id: ViewId,
    shutdown: &'a ShutdownSignal,
}

struct ScanResult {
    books_to_insert: Vec<(Fp, Info)>,
    path_updates: Vec<(Fp, PathBuf, PathBuf)>,
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
        skip(home, settings, ctx, handles_by_fp, handles_by_path),
        fields(total)
    )
)]
fn scan_entries(
    home: &Path,
    entries: &[DirEntry],
    settings: &ImportSettings,
    ctx: &ScanContext<'_>,
    handles_by_fp: &mut FxHashMap<Fp, PathBuf>,
    handles_by_path: &mut FxHashMap<PathBuf, Fp>,
) -> Option<ScanResult> {
    let total = entries.len();
    tracing::Span::current().record("total", total);

    let mut books_to_insert: Vec<(Fp, Info)> = Vec::new();
    let mut path_updates: Vec<(Fp, PathBuf, PathBuf)> = Vec::new();
    let mut books_to_delete: Vec<Fp> = Vec::new();
    let mut pending_relocations: Vec<PendingRelocation> = Vec::new();
    let mut thumbnails_to_delete: Vec<Fp> = Vec::new();

    for (idx, entry) in entries.iter().enumerate() {
        if ctx.shutdown.should_stop() {
            tracing::info!("import scan interrupted by shutdown");
            return None;
        }

        let path = entry.path();
        let relat = path.strip_prefix(home).unwrap_or(path);

        let fp = match path.fingerprint() {
            Ok(fp) => fp,
            Err(e) => {
                error!(path = ?path, error = %e, "failed to compute fingerprint, skipping");
                send_progress(ctx.hub, ctx.notif_id, idx, total);
                continue;
            }
        };

        if handles_by_fp.contains_key(&fp) {
            if relat != handles_by_fp[&fp] {
                debug!(
                    fp = %fp,
                    old_path = %handles_by_fp[&fp].display(),
                    new_path = %relat.display(),
                    "updated book path"
                );
                let old_path = handles_by_fp.remove(&fp).unwrap();
                handles_by_path.remove(&old_path);
                handles_by_fp.insert(fp, relat.to_path_buf());
                handles_by_path.insert(relat.to_path_buf(), fp);
                path_updates.push((fp, relat.to_path_buf(), path.to_path_buf()));
            }
            send_progress(ctx.hub, ctx.notif_id, idx, total);
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
            handles_by_fp.insert(fp, relat.to_path_buf());
            handles_by_path.insert(relat.to_path_buf(), fp);
            books_to_delete.push(old_fp);

            pending_relocations.push(PendingRelocation::FingerprintChanged {
                new_fp: fp,
                old_fp,
                file_size: entry.metadata().map(|m| m.len()).unwrap_or(0),
            });

            thumbnails_to_delete.push(old_fp);
            send_progress(ctx.hub, ctx.notif_id, idx, total);
            continue;
        }

        let kind = file_kind(path).unwrap_or_default();
        if settings.allowed_kinds.contains(&kind) {
            info!(fp = %fp, path = %relat.display(), "added new entry");
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let mut info = Info {
                file: FileInfo {
                    path: relat.to_path_buf(),
                    absolute_path: path.to_path_buf(),
                    kind,
                    size,
                },
                ..Default::default()
            };
            if settings.metadata_kinds.contains(&info.file.kind) {
                extract_metadata_from_document(home, &mut info);
            }
            handles_by_fp.insert(fp, relat.to_path_buf());
            handles_by_path.insert(relat.to_path_buf(), fp);
            books_to_insert.push((fp, info));
        }

        send_progress(ctx.hub, ctx.notif_id, idx, total);
    }

    Some(ScanResult {
        books_to_insert,
        path_updates,
        books_to_delete,
        pending_relocations,
        thumbnails_to_delete,
    })
}

fn send_progress(hub: &Sender<Event>, notif_id: ViewId, idx: usize, total: usize) {
    let Some(percent) = ((idx + 1) * 100).checked_div(total) else {
        return;
    };
    let percent = percent as u8;
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
fn find_deleted_books(handles_by_fp: &FxHashMap<Fp, PathBuf>, home: &Path) -> Vec<Fp> {
    handles_by_fp
        .iter()
        .filter(|(_, relat)| !home.join(relat).exists())
        .map(|(fp, relat)| {
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
    path_updates: Vec<(Fp, PathBuf, PathBuf)>,
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

/// Runs a full directory scan and syncs the database for one library.
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

    let mut handles_by_fp: FxHashMap<Fp, PathBuf> = handles.iter().cloned().collect();
    let mut handles_by_path: FxHashMap<PathBuf, Fp> =
        handles.into_iter().map(|(fp, p)| (p, fp)).collect();

    let entries = walk_files(home);

    let ctx = ScanContext {
        hub,
        notif_id,
        shutdown,
    };

    let Some(mut result) = scan_entries(
        home,
        &entries,
        settings,
        &ctx,
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
    use crate::settings::ImportSettings;
    use crate::task::ShutdownSignal;
    use crate::view::ViewId;
    use std::sync::mpsc;

    fn create_migrated_db() -> Database {
        let db = Database::new(":memory:").expect("in-memory db");
        db.migrate().expect("migrations");
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
}
