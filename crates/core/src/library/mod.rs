pub(crate) mod db;
pub mod importer;
mod migrations;

use crate::db::Database;
use crate::document::SimpleTocEntry;
use crate::helpers::{Fingerprint, Fp};
use crate::library::db::Db as LibraryDb;
use crate::metadata::sorter;
use crate::metadata::{BookQuery, Info, ReaderInfo, SimpleStatus, SortMethod};
use anyhow::{Error, bail, format_err};
use chrono::Local;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

pub(crate) const METADATA_FILENAME: &str = ".metadata.json";
pub(crate) const READING_STATES_DIRNAME: &str = ".reading-states";
#[cfg(not(feature = "test"))]
pub(crate) const THUMBNAIL_PREVIEWS_DIRNAME: &str = ".thumbnail-previews";

#[derive(Debug, Clone, Default)]
pub struct PageResult {
    pub books: Vec<Info>,
    pub total_count: usize,
}

pub struct Library {
    pub home: PathBuf,
    pub db: LibraryDb,
    pub library_id: i64,
    pub sort_method: SortMethod,
    pub reverse_order: bool,
    pub show_hidden: bool,
}

impl Library {
    #[cfg_attr(feature = "tracing", tracing::instrument())]
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

        let sort_method = SortMethod::Opened;

        Ok(Library {
            home: home.as_ref().to_path_buf(),
            db,
            library_id,
            sort_method,
            reverse_order: sort_method.reverse_order(),
            show_hidden: false,
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, query, prefix)))]
    pub fn list<P: AsRef<Path>>(
        &self,
        prefix: P,
        query: Option<&BookQuery>,
        skip_files: bool,
    ) -> (Vec<Info>, BTreeSet<PathBuf>) {
        self.list_by(
            prefix,
            query,
            self.sort_method,
            self.reverse_order,
            skip_files,
        )
    }

    /// Lists books and direct subdirectories under `prefix` using explicit sort parameters.
    ///
    /// When no query is active, sorting is delegated to SQLite. When a query is active it
    /// cannot be expressed in SQL, so books are loaded in full and sorted in Rust.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, query, prefix)))]
    pub fn list_by<P: AsRef<Path>>(
        &self,
        prefix: P,
        query: Option<&BookQuery>,
        sort_method: SortMethod,
        reverse_order: bool,
        skip_files: bool,
    ) -> (Vec<Info>, BTreeSet<PathBuf>) {
        let relat_prefix = prefix
            .as_ref()
            .strip_prefix(&self.home)
            .unwrap_or_else(|_| prefix.as_ref());

        let dirs = self
            .db
            .list_directories_under_prefix(self.library_id, relat_prefix)
            .map_err(|e| {
                error!(error = %e, library_id = self.library_id, "failed to list directories");
            })
            .unwrap_or_default()
            .into_iter()
            .map(|path| prefix.as_ref().join(path))
            .collect();

        if skip_files {
            return (Vec::new(), dirs);
        }

        let files = if query.is_none() {
            self.db
                .page_books(
                    self.library_id,
                    relat_prefix,
                    sort_method,
                    reverse_order,
                    i64::MAX,
                    0,
                )
                .map_err(|e| {
                    error!(error = %e, library_id = self.library_id, "failed to list books");
                })
                .map(|(books, _)| books)
                .unwrap_or_default()
        } else {
            let cmp = sorter(sort_method);
            let mut books: Vec<Info> = self
                .db
                .list_books_under_prefix(self.library_id, relat_prefix)
                .map_err(|e| {
                    error!(error = %e, library_id = self.library_id, "failed to list books");
                })
                .unwrap_or_default()
                .into_iter()
                .filter(|info| query.is_none_or(|q| q.is_match(info)))
                .collect();
            if reverse_order {
                books.sort_by(|a, b| cmp(b, a));
            } else {
                books.sort_by(cmp);
            }
            books
        };

        (files, dirs)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, prefix, query)))]
    pub fn page<P: AsRef<Path>>(
        &self,
        prefix: P,
        query: Option<&BookQuery>,
        page: usize,
        page_size: usize,
    ) -> Result<PageResult, Error> {
        if page_size == 0 {
            return Ok(PageResult::default());
        }

        if query.is_some() {
            let (files, _) = self.list(prefix, query, false);
            let total_count = files.len();
            let start = page.saturating_mul(page_size);
            let books = files.into_iter().skip(start).take(page_size).collect();
            return Ok(PageResult { books, total_count });
        }

        let relat_prefix = prefix
            .as_ref()
            .strip_prefix(&self.home)
            .unwrap_or_else(|_| prefix.as_ref());
        let offset = (page.saturating_mul(page_size)) as i64;
        let limit = page_size as i64;

        let (books, total_count) = self.db.page_books(
            self.library_id,
            relat_prefix,
            self.sort_method,
            self.reverse_order,
            limit,
            offset,
        )?;

        Ok(PageResult {
            books,
            total_count: total_count as usize,
        })
    }

    /// Finds the next or previous results page where the visible status changes.
    ///
    /// When browsing through a paginated list of books, this function helps locate
    /// the boundary page where the [`SimpleStatus`] (New, Reading, or Finished)
    /// changes from one value to another.
    ///
    /// # Arguments
    ///
    /// * `prefix` - Path prefix to filter books within a specific directory
    /// * `query` - Optional filter query to apply (e.g., by title, author)
    /// * `current_page` - The page number we're currently viewing (0-indexed)
    /// * `page_size` - Number of books per page
    /// * `dir` - Direction to search: [`crate::geom::CycleDir::Next`] or [`crate::geom::CycleDir::Previous`]
    ///
    /// # Returns
    ///
    /// `Ok(Some(page_number))` where the status changes, or `Ok(None)` if no
    /// status change is found in that direction.
    ///
    /// # Example
    ///
    /// Suppose books are sorted and paginated with 20 books per page:
    /// - Page 0: books 0-19 (status: New)
    /// - Page 1: books 20-39 (status: New)
    /// - Page 2: books 40-59 (status: Reading)
    /// - Page 3: books 60-79 (status: Finished)
    ///
    /// If currently on page 1 looking for the next status change with
    /// `CycleDir::Next`, the function examines the last book on page 1 (book 19,
    /// status `New`), then scans forward until it finds book 40 with status
    /// `Reading`. It returns `Ok(Some(2))` - the page where the status first
    /// differs.
    ///
    /// Similarly, with `CycleDir::Previous` from page 3, it examines book 60
    /// (status `Finished`) and scans backward to find the boundary, returning
    /// `Ok(Some(2))`.
    ///
    /// Returns `Ok(None)` if there is no status change in the requested direction
    /// (e.g., searching forward from the last page of uniform status).
    pub fn neighbor_status_change_page<P: AsRef<Path>>(
        &self,
        prefix: P,
        query: Option<&BookQuery>,
        current_page: usize,
        page_size: usize,
        dir: crate::geom::CycleDir,
    ) -> Result<Option<usize>, Error> {
        if page_size == 0 {
            return Ok(None);
        }

        let (files, _) = self.list(prefix, query, false);

        if files.is_empty() || current_page >= files.len().div_ceil(page_size) {
            return Ok(None);
        }

        let index_lower = current_page.saturating_mul(page_size);
        let index_upper = (index_lower + page_size).min(files.len());
        if index_lower >= files.len() || index_upper == 0 {
            return Ok(None);
        }

        let book_index = match dir {
            crate::geom::CycleDir::Next => index_upper.saturating_sub(1),
            crate::geom::CycleDir::Previous => index_lower,
        };
        let status = files[book_index].simple_status();

        let page = match dir {
            crate::geom::CycleDir::Next => files[book_index + 1..]
                .iter()
                .position(|info| info.simple_status() != status)
                .map(|delta| current_page + 1 + delta / page_size),
            crate::geom::CycleDir::Previous => files[..book_index]
                .iter()
                .rev()
                .position(|info| info.simple_status() != status)
                .map(|delta| current_page.saturating_sub(1 + delta / page_size)),
        };

        Ok(page)
    }

    pub fn resolve_id(&self) -> (i64, &std::path::Path) {
        (self.library_id, &self.home)
    }

    pub fn add_document(&mut self, info: Info) {
        let path = self.home.join(&info.file.path);
        let fp = match path.fingerprint() {
            Ok(fp) => fp,
            Err(e) => {
                error!(path = %path.display(), error = %e, "failed to fingerprint document");
                return;
            }
        };

        if let Err(e) = self.db.insert_book(self.library_id, fp, &info) {
            error!(fp = %fp, error = %e, "failed to insert book into database");
            return;
        }

        debug!(fp = %fp, title = %info.title, "book inserted into database");

        if let Err(e) = self.db.insert_sort_rank(self.library_id, fp, &info) {
            error!(fp = %fp, error = %e, "failed to insert sort rank for new book");
        }
    }

    pub fn rename<P: AsRef<Path>>(&mut self, path: P, file_name: &str) -> Result<(), Error> {
        let src = self.home.join(path.as_ref());

        let fp = self
            .resolve_fingerprint(path.as_ref())
            .ok_or_else(|| format_err!("can't get fingerprint of {}", path.as_ref().display()))?;

        let mut dest = src.clone();
        dest.set_file_name(file_name);
        fs::rename(&src, &dest)?;

        let new_path = dest.strip_prefix(&self.home)?;

        if let Some(mut info) = self.db.get_book_by_fingerprint(self.library_id, fp)? {
            info.file.path = new_path.to_path_buf();
            info.file.absolute_path = dest.clone();

            if let Err(e) = self.db.update_book(self.library_id, fp, &info) {
                error!(fp = %fp, error = %e, "failed to update book path in database");
            } else {
                debug!(fp = %fp, new_path = %new_path.display(), "book path updated in database");

                if let Err(e) = self.db.insert_sort_rank(self.library_id, fp, &info) {
                    error!(fp = %fp, error = %e, "failed to update sort rank after rename");
                }
            }
        }

        Ok(())
    }

    pub fn remove<P: AsRef<Path>>(&mut self, path: P) -> Result<(), Error> {
        let full_path = self.home.join(path.as_ref());

        let fp = self
            .resolve_fingerprint(path.as_ref())
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

        let fp = self
            .resolve_fingerprint(path.as_ref())
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

        if let Ok(Some(thumbnail_data)) = self.db.get_thumbnail(fp) {
            other.db.save_thumbnail(fp, &thumbnail_data).ok();
        }

        if let Some(mut info) = self.db.get_book_by_fingerprint(self.library_id, fp)? {
            let dest_path = dest.strip_prefix(&other.home)?;
            info.file.path = dest_path.to_path_buf();
            info.file.absolute_path = dest.clone();

            if let Err(e) = other.db.insert_book(other.library_id, fp, &info) {
                error!(fp = %fp, error = %e, "failed to insert copied book into target database");
            } else {
                debug!(fp = %fp, "book copied to target database");

                if let Err(e) = other.db.insert_sort_rank(other.library_id, fp, &info) {
                    error!(fp = %fp, error = %e, "failed to insert sort rank for copied book");
                }
            }
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

        let fp = self
            .resolve_fingerprint(path.as_ref())
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

        if let Some(mut info) = self.db.get_book_by_fingerprint(self.library_id, fp)? {
            let dest_path = dest.strip_prefix(&other.home)?;
            info.file.path = dest_path.to_path_buf();
            info.file.absolute_path = dest.clone();

            if let Err(e) = other.db.insert_book(other.library_id, fp, &info) {
                error!(fp = %fp, error = %e, "failed to insert moved book into target database");
            } else {
                debug!(fp = %fp, "book moved to target database");

                if let Err(e) = other.db.insert_sort_rank(other.library_id, fp, &info) {
                    error!(fp = %fp, error = %e, "failed to insert sort rank for moved book");
                }
            }

            if let Some(thumbnail_data) = thumbnail_data {
                other.db.save_thumbnail(fp, &thumbnail_data).ok();
            }

            if let Err(e) = self.db.delete_book(self.library_id, fp) {
                error!(fp = %fp, error = %e, "failed to delete moved book from source database");
            }
        }

        Ok(())
    }

    /// No-op for the database-backed library: the database maintains its own consistency.
    pub fn clean_up(&mut self) {}

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn set_sort(&mut self, sort_method: SortMethod, reverse_order: bool) {
        self.sort_method = sort_method;
        self.reverse_order = reverse_order;
    }

    pub fn apply<F>(&mut self, f: F)
    where
        F: Fn(&Path, &mut Info),
    {
        let books = match self.db.get_all_books(self.library_id) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "failed to load books for apply");
                return;
            }
        };

        let updated: Vec<Info> = books
            .into_iter()
            .map(|mut info| {
                f(&self.home, &mut info);
                info
            })
            .collect();

        let refs: Vec<(Fp, &Info)> = updated
            .iter()
            .filter_map(|info| info.fp.map(|fp| (fp, info)))
            .collect();

        if let Err(e) = self.db.batch_update_books(self.library_id, &refs) {
            error!(error = %e, "failed to persist apply changes to database");
        }
    }

    pub fn sync_reader_info<P: AsRef<Path>>(&mut self, path: P, reader: &ReaderInfo) {
        let path = path.as_ref();
        let Some(fp) = self.resolve_fingerprint(path) else {
            return;
        };

        if let Err(e) = self.db.save_reading_state(fp, reader) {
            error!(fp = %fp, error = %e, "failed to save reading state to database");
        } else {
            debug!(fp = %fp, "reading state saved to database");
        }
    }

    /// Persist a book's TOC to the database.
    ///
    /// Call this when a TOC has been parsed from a document for the first time
    /// so subsequent opens can serve it from the database without re-parsing.
    pub fn sync_toc<P: AsRef<Path>>(&mut self, path: P, toc: Vec<SimpleTocEntry>) {
        let path = path.as_ref();
        let Some(fp) = self.resolve_fingerprint(path) else {
            return;
        };

        if let Err(e) = self.db.save_toc(fp, &toc) {
            error!(fp = %fp, error = %e, "failed to save TOC to database");
        } else {
            debug!(fp = %fp, entry_count = toc.len(), "TOC saved to database");
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn thumbnail_preview<P: AsRef<Path> + std::fmt::Debug>(
        &self,
        path: P,
    ) -> Option<crate::framebuffer::Pixmap> {
        match self
            .db
            .get_thumbnail_by_path(self.library_id, path.as_ref())
        {
            Ok(Some(data)) => crate::framebuffer::Pixmap::from_png_bytes(&data).ok(),
            Ok(None) => None,
            Err(e) => {
                error!(library_id = self.library_id, path = %path.as_ref().display(), error = %e, "failed to load thumbnail from database");
                None
            }
        }
    }

    pub fn set_status<P: AsRef<Path>>(&mut self, path: P, status: SimpleStatus) {
        let path = path.as_ref();
        let Some(fp) = self.resolve_fingerprint(path) else {
            return;
        };

        match status {
            SimpleStatus::New => {
                if let Err(e) = self.db.delete_reading_state(fp) {
                    error!(fp = %fp, error = %e, "failed to delete reading state from database");
                }
            }
            SimpleStatus::Reading | SimpleStatus::Finished => {
                let current_info = self
                    .db
                    .get_book_by_fingerprint(self.library_id, fp)
                    .ok()
                    .flatten();

                let mut reader_info = current_info
                    .and_then(|info| info.reader)
                    .unwrap_or_default();

                reader_info.finished = status == SimpleStatus::Finished;

                if let Err(e) = self.db.save_reading_state(fp, &reader_info) {
                    error!(fp = %fp, error = %e, "failed to save reading state to database");
                } else {
                    debug!(fp = %fp, finished = reader_info.finished, "reading state updated in database");
                }
            }
        }
    }

    /// No-op: the database is the source of truth and requires no explicit cache reload.
    pub fn reload(&mut self) {}

    pub fn is_empty(&self) -> Option<bool> {
        self.db
            .count_books(self.library_id)
            .ok()
            .map(|count| count == 0)
    }

    pub fn next_book_after(&self, fp: Fp) -> Option<Info> {
        let mut books: Vec<Info> = self
            .db
            .list_books_under_prefix(self.library_id, Path::new(""))
            .ok()?;

        if books.is_empty() {
            return None;
        }

        books.sort_by(|left, right| {
            let ordering = sorter(self.sort_method)(left, right);
            if self.reverse_order {
                ordering.reverse()
            } else {
                ordering
            }
        });

        let current_index = books
            .iter()
            .position(|candidate| candidate.fp == Some(fp))?;
        books.into_iter().nth(current_index + 1)
    }

    pub fn most_recently_opened_reading_book(&self) -> Option<Info> {
        self.db
            .most_recently_opened_reading_book(self.library_id)
            .map_err(|e| {
                error!(error = %e, library_id = self.library_id, "failed to get most recently opened reading book");
            })
            .ok()
            .flatten()
    }

    fn resolve_fingerprint(&self, path: &Path) -> Option<Fp> {
        match self.db.get_book_by_path(self.library_id, path) {
            Ok(Some(info)) => {
                if let Some(fp) = info.fp {
                    return Some(fp);
                }
            }
            Ok(None) => {}
            Err(e) => {
                error!(
                    path = %path.display(),
                    error = %e,
                    "failed to resolve fingerprint from database"
                );
            }
        }

        let full_path = self.home.join(path);

        match full_path.fingerprint() {
            Ok(fp) => Some(fp),
            Err(e) => {
                error!(path = %full_path.display(), error = %e, "failed to fingerprint path");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::geom::CycleDir;
    use crate::metadata::FileInfo;
    use crate::settings::ImportSettings;
    use crate::task::ShutdownSignal;
    use std::str::FromStr;
    use std::sync::mpsc;

    fn setup_library_with_book(
        dir: &Path,
        db: &Database,
        name: &str,
        filename: &str,
    ) -> (Library, PathBuf) {
        let lib = Library::new(dir, db, name).expect("failed to create library");
        fs::write(dir.join(filename), b"dummy book content").expect("failed to write test file");
        let (tx, _rx) = mpsc::channel();
        let notif_id = crate::view::ViewId::MessageNotif(0);
        let shutdown = ShutdownSignal::never();
        importer::run(
            &lib.db,
            lib.library_id,
            dir,
            &ImportSettings::default(),
            false,
            &tx,
            notif_id,
            &shutdown,
        );
        (lib, PathBuf::from(filename))
    }

    fn make_info(path: &str, title: &str, fp: Fp) -> Info {
        Info {
            title: title.to_string(),
            file: FileInfo {
                path: PathBuf::from(path),
                absolute_path: PathBuf::from(format!("/library/{path}")),
                kind: "pdf".to_string(),
                size: 1024,
                mtime: None,
            },
            fp: Some(fp),
            ..Default::default()
        }
    }

    fn make_status_info(path: &str, title: &str, fp: Fp, status: SimpleStatus) -> Info {
        let mut info = make_info(path, title, fp);
        let reader = match status {
            SimpleStatus::New => None,
            SimpleStatus::Reading => Some(ReaderInfo {
                current_page: 1,
                pages_count: 10,
                finished: false,
                ..Default::default()
            }),
            SimpleStatus::Finished => Some(ReaderInfo {
                current_page: 10,
                pages_count: 10,
                finished: true,
                ..Default::default()
            }),
        };
        info.reader = reader.clone();
        info.reader_info = reader;
        info
    }

    #[test]
    fn copy_to_sets_absolute_path_in_destination() {
        let src_dir = tempfile::tempdir().expect("failed to create src temp dir");
        let dst_dir = tempfile::tempdir().expect("failed to create dst temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let (mut src_lib, rel_path) =
            setup_library_with_book(src_dir.path(), &db, "Source", "book.epub");
        let mut dst_lib =
            Library::new(dst_dir.path(), &db, "Destination").expect("failed to create dst lib");

        let src_books = src_lib
            .db
            .get_all_books(src_lib.library_id)
            .expect("failed to get src books");
        assert!(
            !src_books.is_empty(),
            "source library should contain the book"
        );

        src_lib
            .copy_to(&rel_path, &mut dst_lib)
            .expect("copy_to failed");

        let dst_books = dst_lib
            .db
            .get_all_books(dst_lib.library_id)
            .expect("failed to get dst books");

        let dst_info = dst_books
            .into_iter()
            .next()
            .expect("destination library should contain the copied book");

        let expected_abs = dst_dir.path().join(&dst_info.file.path);
        assert_eq!(
            dst_info.file.absolute_path, expected_abs,
            "absolute_path should point to the destination file after copy_to"
        );
        assert!(
            dst_info.file.absolute_path.exists(),
            "absolute_path should point to an existing file"
        );
    }

    #[test]
    fn move_to_sets_absolute_path_in_destination() {
        let src_dir = tempfile::tempdir().expect("failed to create src temp dir");
        let dst_dir = tempfile::tempdir().expect("failed to create dst temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let (mut src_lib, rel_path) =
            setup_library_with_book(src_dir.path(), &db, "Source", "book.epub");
        let mut dst_lib =
            Library::new(dst_dir.path(), &db, "Destination").expect("failed to create dst lib");

        let src_books = src_lib
            .db
            .get_all_books(src_lib.library_id)
            .expect("failed to get src books");
        assert!(
            !src_books.is_empty(),
            "source library should contain the book"
        );

        src_lib
            .move_to(&rel_path, &mut dst_lib)
            .expect("move_to failed");

        let src_books_after = src_lib
            .db
            .get_all_books(src_lib.library_id)
            .expect("failed to get src books after move");
        assert!(
            src_books_after.is_empty(),
            "source library should no longer contain the book after move"
        );

        let dst_books = dst_lib
            .db
            .get_all_books(dst_lib.library_id)
            .expect("failed to get dst books");

        let dst_info = dst_books
            .into_iter()
            .next()
            .expect("destination library should contain the moved book");

        let expected_abs = dst_dir.path().join(&dst_info.file.path);
        assert_eq!(
            dst_info.file.absolute_path, expected_abs,
            "absolute_path should point to the destination file after move_to"
        );
        assert!(
            dst_info.file.absolute_path.exists(),
            "absolute_path should point to an existing file"
        );
    }

    #[test]
    fn neighbor_status_change_page_finds_next_and_previous_boundaries() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Status Library").expect("failed to create library");

        let statuses = [
            SimpleStatus::New,
            SimpleStatus::New,
            SimpleStatus::Reading,
            SimpleStatus::Reading,
            SimpleStatus::Finished,
            SimpleStatus::Finished,
        ];

        for (index, status) in statuses.into_iter().enumerate() {
            let fp = Fp::from_str(&format!("{:016X}", index + 1)).expect("invalid fingerprint");
            let info = make_status_info(
                &format!("book-{}.pdf", index + 1),
                &format!("Book {}", index + 1),
                fp,
                status,
            );
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        assert_eq!(
            lib.neighbor_status_change_page(dir.path(), None, 0, 2, CycleDir::Next)
                .expect("next boundary lookup failed"),
            Some(1)
        );
        assert_eq!(
            lib.neighbor_status_change_page(dir.path(), None, 2, 2, CycleDir::Previous)
                .expect("previous boundary lookup failed"),
            Some(1)
        );
        assert_eq!(
            lib.neighbor_status_change_page(dir.path(), None, 2, 2, CycleDir::Next)
                .expect("terminal next lookup failed"),
            None
        );
        assert_eq!(
            lib.neighbor_status_change_page(dir.path(), None, 0, 0, CycleDir::Next)
                .expect("zero page size lookup failed"),
            None
        );
    }

    #[test]
    fn next_book_after_returns_following_book_in_title_order() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let mut lib =
            Library::new(dir.path(), &db, "Next Book Library").expect("failed to create library");
        lib.sort_method = SortMethod::Title;
        lib.reverse_order = false;

        let alpha_fp = Fp::from_str("0000000000000101").expect("invalid alpha fingerprint");
        let beta_fp = Fp::from_str("0000000000000102").expect("invalid beta fingerprint");
        let gamma_fp = Fp::from_str("0000000000000103").expect("invalid gamma fingerprint");

        for (fp, title, path) in [
            (beta_fp, "Beta", "beta.pdf"),
            (gamma_fp, "Gamma", "gamma.pdf"),
            (alpha_fp, "Alpha", "alpha.pdf"),
        ] {
            let info = make_info(path, title, fp);
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        let next = lib
            .next_book_after(alpha_fp)
            .expect("alpha should have a next book");
        assert_eq!(next.fp, Some(beta_fp));
        assert_eq!(next.title, "Beta");

        let last = lib.next_book_after(gamma_fp);
        assert!(last.is_none(), "last book should not have a successor");

        let missing = lib.next_book_after(
            Fp::from_str("00000000000001FF").expect("invalid missing fingerprint"),
        );
        assert!(missing.is_none(), "missing fingerprint should return none");
    }

    #[test]
    fn compute_sort_keys_assigns_correct_title_ranks() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Sort Keys Library").expect("failed to create library");

        let fp_a = Fp::from_str("0000000000000301").expect("invalid fp");
        let fp_b = Fp::from_str("0000000000000302").expect("invalid fp");
        let fp_c = Fp::from_str("0000000000000303").expect("invalid fp");

        // Insert in non-alphabetical order.
        for (fp, title, path) in [
            (fp_c, "Zebra", "zebra.pdf"),
            (fp_a, "Apple", "apple.pdf"),
            (fp_b, "Mango", "mango.pdf"),
        ] {
            lib.db
                .insert_book(lib.library_id, fp, &make_info(path, title, fp))
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        // Verify title sort order via page_books (ascending = alphabetical).
        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Title,
                false,
                10,
                0,
            )
            .expect("page_books failed");

        assert_eq!(total, 3);
        assert_eq!(
            books.iter().map(|b| b.title.as_str()).collect::<Vec<_>>(),
            vec!["Apple", "Mango", "Zebra"]
        );
    }

    #[test]
    fn page_books_paginates_correctly() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Pagination Library").expect("failed to create library");

        for i in 1u8..=5 {
            let fp = Fp::from_str(&format!("{:016X}", i)).expect("invalid fingerprint");
            let title = format!("Book {:02}", i);
            let path = format!("book{i}.pdf");
            lib.db
                .insert_book(lib.library_id, fp, &make_info(&path, &title, fp))
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        // Page 0 with size 2 should return the first 2 books (title order).
        let (page0, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Title,
                false,
                2,
                0,
            )
            .expect("page_books page 0 failed");
        assert_eq!(total, 5);
        assert_eq!(page0.len(), 2);
        assert_eq!(page0[0].title, "Book 01");
        assert_eq!(page0[1].title, "Book 02");

        // Page 1 with size 2.
        let (page1, _) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Title,
                false,
                2,
                2,
            )
            .expect("page_books page 1 failed");
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].title, "Book 03");
        assert_eq!(page1[1].title, "Book 04");

        // Last page with size 2.
        let (page2, _) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Title,
                false,
                2,
                4,
            )
            .expect("page_books page 2 failed");
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].title, "Book 05");
    }

    #[test]
    fn page_books_reverse_order_reverses_results() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Reverse Library").expect("failed to create library");

        for (fp_hex, title, path) in [
            ("0000000000000401", "Alpha", "alpha.pdf"),
            ("0000000000000402", "Beta", "beta.pdf"),
            ("0000000000000403", "Gamma", "gamma.pdf"),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            lib.db
                .insert_book(lib.library_id, fp, &make_info(path, title, fp))
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, _) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Title,
                true,
                10,
                0,
            )
            .expect("page_books failed");

        assert_eq!(
            books.iter().map(|b| b.title.as_str()).collect::<Vec<_>>(),
            vec!["Gamma", "Beta", "Alpha"]
        );
    }

    #[test]
    fn page_method_uses_db_pagination_without_query() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let mut lib =
            Library::new(dir.path(), &db, "Page Method Library").expect("failed to create library");
        lib.sort_method = SortMethod::Title;
        lib.reverse_order = false;

        for (fp_hex, title, path) in [
            ("0000000000000501", "Charlie", "c.pdf"),
            ("0000000000000502", "Alice", "a.pdf"),
            ("0000000000000503", "Bob", "b.pdf"),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            lib.db
                .insert_book(lib.library_id, fp, &make_info(path, title, fp))
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let result = lib.page(dir.path(), None, 0, 2).expect("page failed");

        assert_eq!(result.total_count, 3);
        assert_eq!(result.books.len(), 2);
        assert_eq!(result.books[0].title, "Alice");
        assert_eq!(result.books[1].title, "Bob");
    }

    #[test]
    fn list_subdirectory_returns_correct_absolute_paths() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Dir Nav Library").expect("failed to create library");

        // Simulate a library with books nested two levels deep.
        for (fp_hex, path, title) in [
            (
                "0000000000001001",
                "fiction/fantasy/book1.pdf",
                "Fantasy One",
            ),
            ("0000000000001002", "fiction/scifi/book2.pdf", "SciFi One"),
            ("0000000000001003", "nonfiction/book3.pdf", "Nonfiction One"),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            lib.db
                .insert_book(lib.library_id, fp, &make_info(path, title, fp))
                .expect("failed to insert book");
        }

        // Listing at root should return top-level dirs as absolute paths.
        let (_, root_dirs) = lib.list(dir.path(), None, true);
        let root_dir_paths: Vec<_> = root_dirs.iter().collect();
        assert_eq!(root_dir_paths.len(), 2);
        assert!(root_dirs.contains(&dir.path().join("fiction")));
        assert!(root_dirs.contains(&dir.path().join("nonfiction")));

        // Listing under "fiction" should return only the immediate subdirs,
        // not double-prefixed paths like /tmp/.../fiction/fiction/fantasy.
        let fiction_prefix = dir.path().join("fiction");
        let (_, fiction_dirs) = lib.list(&fiction_prefix, None, true);
        assert_eq!(
            fiction_dirs.len(),
            2,
            "expected exactly 2 subdirs under fiction"
        );
        assert!(
            fiction_dirs.contains(&fiction_prefix.join("fantasy")),
            "expected fiction/fantasy, got: {fiction_dirs:?}"
        );
        assert!(
            fiction_dirs.contains(&fiction_prefix.join("scifi")),
            "expected fiction/scifi, got: {fiction_dirs:?}"
        );
    }

    #[test]
    fn page_books_status_sort_orders_finished_new_reading() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Status Sort Library").expect("failed to create library");

        let fp_new = Fp::from_str("0000000000000601").expect("invalid fp");
        let fp_reading = Fp::from_str("0000000000000602").expect("invalid fp");
        let fp_finished = Fp::from_str("0000000000000603").expect("invalid fp");

        for (fp, status, path, title) in [
            (fp_new, SimpleStatus::New, "new.pdf", "New Book"),
            (
                fp_reading,
                SimpleStatus::Reading,
                "reading.pdf",
                "Reading Book",
            ),
            (
                fp_finished,
                SimpleStatus::Finished,
                "finished.pdf",
                "Finished Book",
            ),
        ] {
            lib.db
                .insert_book(
                    lib.library_id,
                    fp,
                    &make_status_info(path, title, fp, status),
                )
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Status,
                false,
                10,
                0,
            )
            .expect("page_books with Status sort failed");

        assert_eq!(total, 3);
        // Status ASC: Finished(0) < New(1) < Reading(2).
        assert_eq!(books[0].title, "Finished Book");
        assert_eq!(books[1].title, "New Book");
        assert_eq!(books[2].title, "Reading Book");
    }

    #[test]
    fn page_books_progress_sort_orders_by_completion() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib = Library::new(dir.path(), &db, "Progress Sort Library")
            .expect("failed to create library");

        let fp_new = Fp::from_str("0000000000000701").expect("invalid fp");
        let fp_halfway = Fp::from_str("0000000000000702").expect("invalid fp");
        let fp_finished = Fp::from_str("0000000000000703").expect("invalid fp");

        let mut info_halfway = make_info("halfway.pdf", "Halfway Book", fp_halfway);
        info_halfway.reader_info = Some(ReaderInfo {
            current_page: 5,
            pages_count: 10,
            finished: false,
            ..Default::default()
        });
        info_halfway.reader = info_halfway.reader_info.clone();

        for (fp, info) in [
            (
                fp_new,
                make_status_info("new.pdf", "New Book", fp_new, SimpleStatus::New),
            ),
            (fp_halfway, info_halfway),
            (
                fp_finished,
                make_status_info(
                    "finished.pdf",
                    "Finished Book",
                    fp_finished,
                    SimpleStatus::Finished,
                ),
            ),
        ] {
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Progress,
                false,
                10,
                0,
            )
            .expect("page_books with Progress sort failed");

        assert_eq!(total, 3);
        // Progress ASC: Finished(0) < New(1) < Reading-with-progress(2).
        assert_eq!(books[0].title, "Finished Book");
        assert_eq!(books[1].title, "New Book");
        assert_eq!(books[2].title, "Halfway Book");
    }

    #[test]
    fn page_books_pages_sort_orders_by_page_count() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Pages Sort Library").expect("failed to create library");

        for (fp_hex, path, title, pages) in [
            ("0000000000000801", "big.pdf", "Big Book", 500usize),
            ("0000000000000802", "tiny.pdf", "Tiny Book", 50),
            ("0000000000000803", "medium.pdf", "Medium Book", 200),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            let mut info = make_info(path, title, fp);
            info.reader_info = Some(ReaderInfo {
                pages_count: pages,
                ..Default::default()
            });
            info.reader = info.reader_info.clone();
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Pages,
                false,
                10,
                0,
            )
            .expect("page_books with Pages sort failed");

        assert_eq!(total, 3);
        assert_eq!(books[0].title, "Tiny Book");
        assert_eq!(books[1].title, "Medium Book");
        assert_eq!(books[2].title, "Big Book");
    }

    #[test]
    fn page_books_size_sort_orders_by_file_size() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Size Sort Library").expect("failed to create library");

        for (fp_hex, path, title, size) in [
            ("0000000000000901", "big.pdf", "Big Book", 9000u64),
            ("0000000000000902", "tiny.pdf", "Tiny Book", 100),
            ("0000000000000903", "medium.pdf", "Medium Book", 4500),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            let mut info = make_info(path, title, fp);
            info.file.size = size;
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Size,
                false,
                10,
                0,
            )
            .expect("page_books with Size sort failed");

        assert_eq!(total, 3);
        assert_eq!(books[0].title, "Tiny Book");
        assert_eq!(books[1].title, "Medium Book");
        assert_eq!(books[2].title, "Big Book");
    }

    #[test]
    fn page_books_kind_sort_orders_alphabetically_by_file_kind() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Kind Sort Library").expect("failed to create library");

        for (fp_hex, path, title, kind) in [
            ("0000000000000A01", "book.pdf", "PDF Book", "pdf"),
            ("0000000000000A02", "book.epub", "EPUB Book", "epub"),
            ("0000000000000A03", "book.cbz", "CBZ Book", "cbz"),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            let mut info = make_info(path, title, fp);
            info.file.kind = kind.to_string();
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Kind,
                false,
                10,
                0,
            )
            .expect("page_books with Kind sort failed");

        assert_eq!(total, 3);
        // Alphabetical: cbz < epub < pdf.
        assert_eq!(books[0].title, "CBZ Book");
        assert_eq!(books[1].title, "EPUB Book");
        assert_eq!(books[2].title, "PDF Book");
    }

    #[test]
    fn page_books_added_sort_orders_by_insertion_time() {
        use chrono::NaiveDateTime;
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Added Sort Library").expect("failed to create library");

        let t0 = NaiveDateTime::parse_from_str("2020-01-01 00:00:00", "%Y-%m-%d %H:%M:%S")
            .expect("invalid datetime");
        let t1 = NaiveDateTime::parse_from_str("2021-06-15 12:00:00", "%Y-%m-%d %H:%M:%S")
            .expect("invalid datetime");
        let t2 = NaiveDateTime::parse_from_str("2023-03-20 08:30:00", "%Y-%m-%d %H:%M:%S")
            .expect("invalid datetime");

        for (fp_hex, path, title, added) in [
            ("0000000000000B01", "old.pdf", "Old Book", t0),
            ("0000000000000B02", "recent.pdf", "Recent Book", t2),
            ("0000000000000B03", "mid.pdf", "Middle Book", t1),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            let mut info = make_info(path, title, fp);
            info.added = added;
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Added,
                false,
                10,
                0,
            )
            .expect("page_books with Added sort failed");

        assert_eq!(total, 3);
        assert_eq!(books[0].title, "Old Book");
        assert_eq!(books[1].title, "Middle Book");
        assert_eq!(books[2].title, "Recent Book");
    }

    #[test]
    fn page_books_opened_sort_orders_by_last_opened_time() {
        use chrono::NaiveDateTime;
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Opened Sort Library").expect("failed to create library");

        let t0 = NaiveDateTime::parse_from_str("2020-01-01 00:00:00", "%Y-%m-%d %H:%M:%S")
            .expect("invalid datetime");
        let t1 = NaiveDateTime::parse_from_str("2022-09-10 09:00:00", "%Y-%m-%d %H:%M:%S")
            .expect("invalid datetime");
        let t2 = NaiveDateTime::parse_from_str("2024-04-01 17:45:00", "%Y-%m-%d %H:%M:%S")
            .expect("invalid datetime");

        for (fp_hex, path, title, opened) in [
            ("0000000000000C01", "oldest.pdf", "Oldest Opened", t0),
            ("0000000000000C02", "newest.pdf", "Newest Opened", t2),
            ("0000000000000C03", "middle.pdf", "Middle Opened", t1),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            let mut info = make_info(path, title, fp);
            info.reader_info = Some(ReaderInfo {
                opened,
                pages_count: 100,
                ..Default::default()
            });
            info.reader = info.reader_info.clone();
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Opened,
                false,
                10,
                0,
            )
            .expect("page_books with Opened sort failed");

        assert_eq!(total, 3);
        assert_eq!(books[0].title, "Oldest Opened");
        assert_eq!(books[1].title, "Middle Opened");
        assert_eq!(books[2].title, "Newest Opened");
    }

    #[test]
    fn page_books_year_sort_orders_by_publication_year() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Year Sort Library").expect("failed to create library");

        for (fp_hex, path, title, year) in [
            ("0000000000000D01", "modern.pdf", "Modern Book", "2020"),
            ("0000000000000D02", "old.pdf", "Old Book", "1990"),
            ("0000000000000D03", "ancient.pdf", "Ancient Book", "1850"),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            let mut info = make_info(path, title, fp);
            info.year = year.to_string();
            lib.db
                .insert_book(lib.library_id, fp, &info)
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new(""),
                SortMethod::Year,
                false,
                10,
                0,
            )
            .expect("page_books with Year sort failed");

        assert_eq!(total, 3);
        assert_eq!(books[0].title, "Ancient Book");
        assert_eq!(books[1].title, "Old Book");
        assert_eq!(books[2].title, "Modern Book");
    }

    #[test]
    fn page_books_count_query_respects_prefix_filter() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib = Library::new(dir.path(), &db, "Prefix Count Library")
            .expect("failed to create library");

        for (fp_hex, path, title) in [
            ("0000000000000E01", "fiction/book1.pdf", "Fiction One"),
            ("0000000000000E02", "fiction/book2.pdf", "Fiction Two"),
            ("0000000000000E03", "nonfiction/book3.pdf", "Nonfiction One"),
        ] {
            let fp = Fp::from_str(fp_hex).expect("invalid fp");
            lib.db
                .insert_book(lib.library_id, fp, &make_info(path, title, fp))
                .expect("failed to insert book");
        }

        lib.db
            .compute_sort_keys(lib.library_id)
            .expect("compute_sort_keys failed");

        let (books, total) = lib
            .db
            .page_books(
                lib.library_id,
                Path::new("fiction"),
                SortMethod::Title,
                false,
                10,
                0,
            )
            .expect("page_books with prefix filter failed");

        assert_eq!(total, 2, "count query should reflect the prefix filter");
        assert_eq!(books.len(), 2);
        assert!(books.iter().all(|b| b.title.starts_with("Fiction")));
    }

    #[test]
    fn resolve_fingerprint_prefers_db_and_falls_back_to_filesystem() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let lib =
            Library::new(dir.path(), &db, "Fingerprint Library").expect("failed to create library");

        let stored_fp = Fp::from_str("0000000000000201").expect("invalid stored fingerprint");
        let stored_info = make_info("stored.pdf", "Stored", stored_fp);
        lib.db
            .insert_book(lib.library_id, stored_fp, &stored_info)
            .expect("failed to insert stored book");

        assert_eq!(
            lib.resolve_fingerprint(Path::new("stored.pdf")),
            Some(stored_fp)
        );

        let fallback_path = dir.path().join("fallback.pdf");
        fs::write(&fallback_path, b"fallback content").expect("failed to write fallback file");
        let expected_fallback_fp = fallback_path
            .fingerprint()
            .expect("failed to fingerprint fallback file");

        assert_eq!(
            lib.resolve_fingerprint(Path::new("fallback.pdf")),
            Some(expected_fallback_fp)
        );
        assert_eq!(lib.resolve_fingerprint(Path::new("missing.pdf")), None);
    }
}
