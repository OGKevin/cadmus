mod db;
mod migrations;

use crate::db::Database;
use crate::document::{file_kind, SimpleTocEntry};
use crate::helpers::{Fingerprint, Fp, IsHidden};
use crate::library::db::Db as LibraryDb;
use crate::metadata::extract_metadata_from_document;
use crate::metadata::sorter;
use crate::metadata::{BookQuery, FileInfo, Info, ReaderInfo, SimpleStatus, SortMethod};
use crate::settings::ImportSettings;
use anyhow::{bail, format_err, Error};
use chrono::Local;
use fxhash::{FxBuildHasher, FxHashMap};
use indexmap::IndexMap;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{debug, error, info};
use walkdir::WalkDir;

const METADATA_FILENAME: &str = ".metadata.json";
const FAT32_EPOCH_FILENAME: &str = ".fat32-epoch";
const READING_STATES_DIRNAME: &str = ".reading-states";
#[cfg(not(feature = "test"))]
const THUMBNAIL_PREVIEWS_DIRNAME: &str = ".thumbnail-previews";

pub struct Library {
    pub home: PathBuf,
    pub db: LibraryDb,
    pub library_id: i64,
    /// In-memory cache of book metadata keyed by fingerprint.
    ///
    /// SQLite is the source of truth, but the UI and library operations rely on
    /// fast iteration, ordering, and path/fingerprint lookups, so we keep a
    /// cache of the current library view in memory.
    pub books: IndexMap<Fp, Info, FxBuildHasher>,
    /// Reverse index for quick path-to-fingerprint lookups.
    ///
    /// This is derived from the cached entries in `books` and is rebuilt on reload.
    pub paths: FxHashMap<PathBuf, Fp>,
    pub fat32_epoch: SystemTime,
    pub sort_method: SortMethod,
    pub reverse_order: bool,
    pub show_hidden: bool,
}

impl Library {
    #[cfg_attr(feature = "otel", tracing::instrument())]
    pub fn new<P: AsRef<Path> + std::fmt::Debug>(
        home: P,
        database: &Database,
        name: &str,
    ) -> Result<Self, Error> {
        let db = LibraryDb::new(database);

        if let Err(e) = fs::create_dir(&home) {
            if e.kind() != ErrorKind::AlreadyExists {
                bail!(e);
            }
        }

        let home_path = home.as_ref().to_path_buf();
        let home_path_str = home_path.to_string_lossy();

        let library_id = if let Some(id) = db.get_library_by_path(&home_path_str)? {
            info!(library_id = id, path = ?home_path, "found existing library");
            id
        } else {
            let id = db.register_library(&home_path_str, name)?;
            info!(library_id = id, path = ?home_path, name = %name, "registered new library");
            id
        };

        info!(library_id, "loading books from database into cache");
        let books = db.get_all_books(library_id)?;
        info!(
            library_id,
            count = books.len(),
            "loaded books from database"
        );

        let mut book_cache =
            IndexMap::with_capacity_and_hasher(books.len(), FxBuildHasher::default());
        let mut paths = FxHashMap::default();

        for (fp, info) in books {
            paths.insert(info.file.path.clone(), fp);
            book_cache.insert(fp, info);
        }

        let path = home.as_ref().join(FAT32_EPOCH_FILENAME);
        if !path.exists() {
            let file = File::create(&path)?;
            file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(315_532_800))?;
        }

        let fat32_epoch = path.metadata()?.modified()?;

        let sort_method = SortMethod::Opened;

        Ok(Library {
            home: home.as_ref().to_path_buf(),
            db,
            library_id,
            books: book_cache,
            paths,
            fat32_epoch,
            sort_method,
            reverse_order: sort_method.reverse_order(),
            show_hidden: false,
        })
    }

    pub fn list<P: AsRef<Path>>(
        &self,
        prefix: P,
        query: Option<&BookQuery>,
        skip_files: bool,
    ) -> (Vec<Info>, BTreeSet<PathBuf>) {
        let mut dirs = BTreeSet::new();
        let mut files = Vec::new();

        let relat_prefix = prefix
            .as_ref()
            .strip_prefix(&self.home)
            .unwrap_or_else(|_| prefix.as_ref());
        for (_, info) in self.books.iter() {
            if let Ok(relat) = info.file.path.strip_prefix(relat_prefix) {
                let mut compos = relat.components();
                let mut first = compos.next();
                if compos.next().is_none() {
                    first = None;
                }
                if let Some(child) = first {
                    dirs.insert(prefix.as_ref().join(child.as_os_str()));
                }
                if skip_files {
                    continue;
                }
                if query.is_none_or(|q| q.is_match(info)) {
                    files.push(info.clone());
                }
            }
        }

        (files, dirs)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, settings)))]
    pub fn import(&mut self, settings: &ImportSettings) {
        let mut books_to_insert = Vec::new();
        let mut books_to_update = Vec::new();
        let mut books_to_delete = Vec::new();

        #[cfg(feature = "otel")]
        let _walk_span = tracing::info_span!("walk_directory").entered();

        let walk_entries: Vec<_> = WalkDir::new(&self.home)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !e.is_hidden())
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_type().is_dir())
            .collect();

        #[cfg(feature = "otel")]
        let _walk_span = _walk_span.exit();

        #[cfg(feature = "otel")]
        let _process_span =
            tracing::info_span!("process_entries", count = walk_entries.len()).entered();

        for entry in walk_entries {
            let path = entry.path();
            let relat = path.strip_prefix(&self.home).unwrap_or(path);
            let md = entry.metadata().unwrap();
            let fp = md.fingerprint(self.fat32_epoch).unwrap();

            if self.books.contains_key(&fp) {
                if relat != self.books[&fp].file.path {
                    debug!(
                        "Update path for {}: {} → {}.",
                        fp,
                        self.books[&fp].file.path.display(),
                        relat.display()
                    );
                    self.paths.remove(&self.books[&fp].file.path);
                    self.paths.insert(relat.to_path_buf(), fp);
                    self.books[&fp].file.path = relat.to_path_buf();
                    books_to_update.push(fp);
                }
            } else if let Some(fp2) = self.paths.get(relat).cloned() {
                debug!(
                    "Update fingerprint for {}: {} → {}.",
                    relat.display(),
                    fp2,
                    fp
                );

                books_to_delete.push(fp2);

                let mut info = self.books.swap_remove(&fp2).unwrap();

                if settings.sync_metadata && settings.metadata_kinds.contains(&info.file.kind) {
                    extract_metadata_from_document(&self.home, &mut info);
                }

                info.file.size = md.len();

                self.books.insert(fp, info);
                self.paths.insert(relat.to_path_buf(), fp);
                books_to_insert.push(fp);

                self.db.delete_thumbnail(fp2).ok();
            } else {
                let fp1 = self
                    .fat32_epoch
                    .checked_sub(Duration::from_secs(1))
                    .and_then(|epoch| md.fingerprint(epoch).ok())
                    .unwrap_or(fp);
                let fp2 = self
                    .fat32_epoch
                    .checked_add(Duration::from_secs(1))
                    .and_then(|epoch| md.fingerprint(epoch).ok())
                    .unwrap_or(fp);

                let nfp = if fp1 != fp && self.books.contains_key(&fp1) {
                    Some(fp1)
                } else if fp2 != fp && self.books.contains_key(&fp2) {
                    Some(fp2)
                } else {
                    None
                };

                if let Some(nfp) = nfp {
                    debug!(
                        "Update fingerprint for {}: {} → {}.",
                        self.books[&nfp].file.path.display(),
                        nfp,
                        fp
                    );

                    books_to_delete.push(nfp);

                    let info = self.books.swap_remove(&nfp).unwrap();
                    self.books.insert(fp, info);
                    books_to_insert.push(fp);

                    self.db.move_thumbnail(nfp, fp).ok();
                    if relat != self.books[&fp].file.path {
                        debug!(
                            "Update path for {}: {} → {}.",
                            fp,
                            self.books[&fp].file.path.display(),
                            relat.display()
                        );
                        self.paths.remove(&self.books[&fp].file.path);
                        self.paths.insert(relat.to_path_buf(), fp);
                        self.books[&fp].file.path = relat.to_path_buf();
                        books_to_update.push(fp);
                    }
                } else {
                    let kind = file_kind(path).unwrap_or_default();
                    if !settings.allowed_kinds.contains(&kind) {
                        continue;
                    }
                    info!("Add new entry: {}, {}.", fp, relat.display());
                    let size = md.len();
                    let file = FileInfo {
                        path: relat.to_path_buf(),
                        kind,
                        size,
                    };
                    let mut info = Info {
                        file,
                        ..Default::default()
                    };

                    if settings.metadata_kinds.contains(&info.file.kind) {
                        extract_metadata_from_document(&self.home, &mut info);
                    }

                    self.books.insert(fp, info);
                    self.paths.insert(relat.to_path_buf(), fp);
                    books_to_insert.push(fp);
                }
            }
        }

        #[cfg(feature = "otel")]
        let _process_span = _process_span.exit();

        #[cfg(feature = "otel")]
        let _cleanup_span = tracing::info_span!("cleanup_orphaned_entries").entered();

        let home = &self.home;
        let mut deleted_fps = Vec::new();

        self.books.retain(|fp, info| {
            let path = home.join(&info.file.path);
            if path.exists() {
                true
            } else {
                info!("Remove entry: {}, {}.", fp, info.file.path.display());
                deleted_fps.push(*fp);
                false
            }
        });

        books_to_delete.extend(deleted_fps.iter().copied());

        for fp in &deleted_fps {
            self.paths.retain(|_, path_fp| path_fp != fp);
        }

        #[cfg(feature = "otel")]
        let _cleanup_span = _cleanup_span.exit();

        #[cfg(feature = "otel")]
        let _db_span = tracing::info_span!("database_batch_operations").entered();

        if !books_to_insert.is_empty() {
            let book_refs: Vec<(Fp, &Info)> = books_to_insert
                .iter()
                .filter_map(|fp| self.books.get(fp).map(|info| (*fp, info)))
                .collect();

            if let Err(e) = self.db.batch_insert_books(self.library_id, &book_refs) {
                error!(
                    error = %e,
                    count = book_refs.len(),
                    "batch insert failed"
                );
            }
        }

        if !books_to_update.is_empty() {
            let book_refs: Vec<(Fp, &Info)> = books_to_update
                .iter()
                .filter_map(|fp| self.books.get(fp).map(|info| (*fp, info)))
                .collect();

            if let Err(e) = self.db.batch_update_books(&book_refs) {
                error!(
                    error = %e,
                    count = book_refs.len(),
                    "batch update failed"
                );
            }
        }

        if !books_to_delete.is_empty() {
            if let Err(e) = self
                .db
                .batch_delete_books(self.library_id, &books_to_delete)
            {
                error!(
                    error = %e,
                    count = books_to_delete.len(),
                    "batch delete failed"
                );
            }
        }
    }

    pub fn add_document(&mut self, info: Info) {
        let path = self.home.join(&info.file.path);
        let md = path.metadata().unwrap();
        let fp = md.fingerprint(self.fat32_epoch).unwrap();

        if let Err(e) = self.db.insert_book(self.library_id, fp, &info) {
            error!(fp = %fp, error = %e, "failed to insert book into database");
        } else {
            debug!(fp = %fp, title = %info.title, "book inserted into database");
        }

        self.paths.insert(info.file.path.clone(), fp);
        self.books.insert(fp, info);
    }

    pub fn rename<P: AsRef<Path>>(&mut self, path: P, file_name: &str) -> Result<(), Error> {
        let src = self.home.join(path.as_ref());

        let fp = self
            .paths
            .remove(path.as_ref())
            .or_else(|| {
                src.metadata()
                    .ok()
                    .and_then(|md| md.fingerprint(self.fat32_epoch).ok())
            })
            .ok_or_else(|| format_err!("can't get fingerprint of {}", path.as_ref().display()))?;

        let mut dest = src.clone();
        dest.set_file_name(file_name);
        fs::rename(&src, &dest)?;

        let new_path = dest.strip_prefix(&self.home)?;
        self.paths.insert(new_path.to_path_buf(), fp);
        if let Some(info) = self.books.get_mut(&fp) {
            info.file.path = new_path.to_path_buf();

            if let Err(e) = self.db.update_book(fp, info) {
                error!(fp = %fp, error = %e, "failed to update book path in database");
            } else {
                debug!(fp = %fp, new_path = %new_path.display(), "book path updated in database");
            }
        }

        Ok(())
    }

    pub fn remove<P: AsRef<Path>>(&mut self, path: P) -> Result<(), Error> {
        let full_path = self.home.join(path.as_ref());

        let fp = self
            .paths
            .get(path.as_ref())
            .cloned()
            .or_else(|| {
                full_path
                    .metadata()
                    .ok()
                    .and_then(|md| md.fingerprint(self.fat32_epoch).ok())
            })
            .ok_or_else(|| format_err!("can't get fingerprint of {}", path.as_ref().display()))?;

        if full_path.exists() {
            fs::remove_file(&full_path)?;
        }

        if let Some(parent) = full_path.parent() {
            if parent != self.home {
                fs::remove_dir(parent).ok();
            }
        }

        self.db.delete_thumbnail(fp).ok();

        if let Err(e) = self.db.delete_book(self.library_id, fp) {
            error!(fp = %fp, error = %e, "failed to delete book from database");
        } else {
            debug!(fp = %fp, "book deleted from database");
        }

        self.paths.remove(path.as_ref());
        self.books.shift_remove(&fp);

        Ok(())
    }

    pub fn copy_to<P: AsRef<Path>>(&mut self, path: P, other: &mut Library) -> Result<(), Error> {
        let src = self.home.join(path.as_ref());

        if !src.exists() {
            return Err(format_err!(
                "can't copy non-existing file {}",
                path.as_ref().display()
            ));
        }

        let md = src.metadata()?;
        let fp = self
            .paths
            .get(path.as_ref())
            .cloned()
            .or_else(|| md.fingerprint(self.fat32_epoch).ok())
            .ok_or_else(|| format_err!("can't get fingerprint of {}", path.as_ref().display()))?;

        let mut dest = other.home.join(path.as_ref());
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        if dest.exists() {
            let prefix = Local::now().format("%Y%m%d_%H%M%S ");
            let name = dest
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| prefix.to_string() + name)
                .ok_or_else(|| format_err!("can't compute new name for {}", dest.display()))?;
            dest.set_file_name(name);
        }

        fs::copy(&src, &dest)?;
        {
            let fdest = File::open(&dest)?;
            fdest.set_modified(md.modified()?)?;
        }

        if let Ok(Some(thumbnail_data)) = self.db.get_thumbnail(fp) {
            other.db.save_thumbnail(fp, &thumbnail_data).ok();
        }

        let info = self.books.get(&fp).cloned();
        if let Some(mut info) = info {
            let dest_path = dest.strip_prefix(&other.home)?;
            info.file.path = dest_path.to_path_buf();

            if let Err(e) = other.db.insert_book(other.library_id, fp, &info) {
                error!(fp = %fp, error = %e, "failed to insert copied book into target database");
            } else {
                debug!(fp = %fp, "book copied to target database");
            }

            other.books.insert(fp, info);
            other.paths.insert(dest_path.to_path_buf(), fp);
        }

        Ok(())
    }

    pub fn move_to<P: AsRef<Path>>(&mut self, path: P, other: &mut Library) -> Result<(), Error> {
        let src = self.home.join(path.as_ref());

        if !src.exists() {
            return Err(format_err!(
                "can't move non-existing file {}",
                path.as_ref().display()
            ));
        }

        let md = src.metadata()?;
        let fp = self
            .paths
            .get(path.as_ref())
            .cloned()
            .or_else(|| md.fingerprint(self.fat32_epoch).ok())
            .ok_or_else(|| format_err!("can't get fingerprint of {}", path.as_ref().display()))?;

        let src = self.home.join(path.as_ref());
        let mut dest = other.home.join(path.as_ref());
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        if dest.exists() {
            let prefix = Local::now().format("%Y%m%d_%H%M%S ");
            let name = dest
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| prefix.to_string() + name)
                .ok_or_else(|| format_err!("can't compute new name for {}", dest.display()))?;
            dest.set_file_name(name);
        }

        fs::rename(&src, &dest)?;

        let thumbnail_data = self.db.get_thumbnail(fp).ok().flatten();

        let info = self.books.shift_remove(&fp);
        if let Some(mut info) = info {
            let dest_path = dest.strip_prefix(&other.home)?;
            info.file.path = dest_path.to_path_buf();

            if let Err(e) = other.db.insert_book(other.library_id, fp, &info) {
                error!(fp = %fp, error = %e, "failed to insert moved book into target database");
            } else {
                debug!(fp = %fp, "book moved to target database");
            }

            if let Some(thumbnail_data) = thumbnail_data {
                other.db.save_thumbnail(fp, &thumbnail_data).ok();
            }

            if let Err(e) = self.db.delete_book(self.library_id, fp) {
                error!(fp = %fp, error = %e, "failed to delete moved book from source database");
            }

            other.books.insert(fp, info);
            self.paths.remove(path.as_ref());
            other.paths.insert(dest_path.to_path_buf(), fp);
        }

        Ok(())
    }

    /// No-op for the database-backed library: the database maintains its own consistency.
    pub fn clean_up(&mut self) {}

    pub fn sort(&mut self, sort_method: SortMethod, reverse_order: bool) {
        self.sort_method = sort_method;
        self.reverse_order = reverse_order;

        let sort_fn = sorter(sort_method);

        if reverse_order {
            self.books.sort_by(|_, a, _, b| sort_fn(a, b).reverse());
        } else {
            self.books.sort_by(|_, a, _, b| sort_fn(a, b));
        }
    }

    pub fn apply<F>(&mut self, f: F)
    where
        F: Fn(&Path, &mut Info),
    {
        for (_, info) in &mut self.books {
            f(&self.home, info);
        }
    }

    pub fn sync_reader_info<P: AsRef<Path>>(&mut self, path: P, reader: &ReaderInfo) {
        let fp = self.paths.get(path.as_ref()).cloned().unwrap_or_else(|| {
            self.home
                .join(path.as_ref())
                .metadata()
                .unwrap()
                .fingerprint(self.fat32_epoch)
                .unwrap()
        });

        if let Err(e) = self.db.save_reading_state(fp, reader) {
            error!(fp = %fp, error = %e, "failed to save reading state to database");
        } else {
            debug!(fp = %fp, "reading state saved to database");
        }

        if let Some(info) = self.books.get_mut(&fp) {
            info.reader = Some(reader.clone());
        }
    }

    /// Persist a book's TOC to the database and update the in-memory entry.
    ///
    /// Call this when a TOC has been parsed from a document for the first time
    /// so subsequent opens can serve it from the database without re-parsing.
    pub fn sync_toc<P: AsRef<Path>>(&mut self, path: P, toc: Vec<SimpleTocEntry>) {
        let fp = self.paths.get(path.as_ref()).cloned().unwrap_or_else(|| {
            self.home
                .join(path.as_ref())
                .metadata()
                .unwrap()
                .fingerprint(self.fat32_epoch)
                .unwrap()
        });

        if let Err(e) = self.db.save_toc(fp, &toc) {
            error!(fp = %fp, error = %e, "failed to save TOC to database");
        } else {
            debug!(fp = %fp, entry_count = toc.len(), "TOC saved to database");
        }

        if let Some(info) = self.books.get_mut(&fp) {
            info.toc = Some(toc);
        }
    }

    pub fn thumbnail_preview<P: AsRef<Path>>(&self, path: P) -> Option<crate::framebuffer::Pixmap> {
        let fp = self.fingerprint_for_path(path.as_ref())?;
        match self.db.get_thumbnail(fp) {
            Ok(Some(data)) => crate::framebuffer::Pixmap::from_png_bytes(&data).ok(),
            Ok(None) => None,
            Err(e) => {
                error!(fp = %fp, error = %e, "failed to load thumbnail from database");
                None
            }
        }
    }

    pub fn set_status<P: AsRef<Path>>(&mut self, path: P, status: SimpleStatus) {
        let fp = self.paths.get(path.as_ref()).cloned().unwrap_or_else(|| {
            self.home
                .join(path.as_ref())
                .metadata()
                .unwrap()
                .fingerprint(self.fat32_epoch)
                .unwrap()
        });

        match status {
            SimpleStatus::New => {
                if let Some(info) = self.books.get_mut(&fp) {
                    info.reader = None;
                }

                if let Err(e) = self.db.delete_reading_state(fp) {
                    error!(fp = %fp, error = %e, "failed to delete reading state from database");
                }
            }
            SimpleStatus::Reading | SimpleStatus::Finished => {
                if let Some(info) = self.books.get_mut(&fp) {
                    let reader_info = info.reader.get_or_insert_with(ReaderInfo::default);
                    reader_info.finished = status == SimpleStatus::Finished;

                    if let Err(e) = self.db.save_reading_state(fp, reader_info) {
                        error!(fp = %fp, error = %e, "failed to save reading state to database");
                    } else {
                        debug!(fp = %fp, finished = reader_info.finished, "reading state updated in database");
                    }
                }
            }
        }
    }

    pub fn reload(&mut self) {
        self.books.clear();
        self.paths.clear();

        match self.db.get_all_books(self.library_id) {
            Err(e) => {
                error!(error = %e, "failed to reload books from database");
            }
            Ok(books) => {
                debug!(count = books.len(), "reloaded books from database");
                for (fp, info) in books {
                    self.paths.insert(info.file.path.clone(), fp);
                    self.books.insert(fp, info);
                }
            }
        }
    }

    /// No-op: database writes are immediate and do not require an explicit flush.
    pub fn flush(&mut self) {}

    pub fn is_empty(&self) -> Option<bool> {
        Some(self.books.is_empty())
    }

    fn fingerprint_for_path(&self, path: &Path) -> Option<Fp> {
        self.paths.get(path).cloned().or_else(|| {
            self.home
                .join(path)
                .metadata()
                .ok()
                .and_then(|md| md.fingerprint(self.fat32_epoch).ok())
        })
    }
}
