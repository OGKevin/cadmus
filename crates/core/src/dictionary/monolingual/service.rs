//! High-level service for monolingual dictionary management.
//!
//! [`MonolingualDictionaryService`] is the single public entry point for all
//! monolingual dictionary operations: querying the remote catalogue, listing
//! installed dictionaries, and installing a new one.

use super::client::MonolingualClient;
use super::db::Db;
use super::errors::MonolingualError;
use super::metadata::{DictionariesResponse, DictionaryEntry, download_url, download_url_no_etym};
use crate::db::Database;
use std::collections::HashSet;
use std::fs;
use std::io::{self};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use zip::ZipArchive;

/// Subdirectory inside the dictionaries root where reader-dict downloads live.
const READER_DICT_SUBDIR: &str = "reader-dict";

/// Provides monolingual dictionary management: querying available dictionaries,
/// listing installed ones, and downloading + extracting new ones.
///
/// All network metadata is cached in the application SQLite database.
/// Downloaded dictionaries are extracted to
/// `<dict_dir>/reader-dict/<lang>/`.
///
/// The service is cheaply cloneable (`Arc`-backed). All clones share the same
/// `pending_installs` set, so concurrent-download guards work correctly across
/// the UI thread (which holds the original) and background threads (which hold
/// clones).
#[derive(Clone, Debug)]
pub struct MonolingualDictionaryService {
    client: MonolingualClient,
    db: Db,
    dict_dir: PathBuf,
    pending_installs: Arc<Mutex<HashSet<String>>>,
}

impl MonolingualDictionaryService {
    /// Creates a new service.
    ///
    /// # Arguments
    ///
    /// * `database` - Application SQLite database used for metadata caching.
    /// * `dict_dir` - Root directory where dictionaries are stored. Downloads
    ///   are placed in `<dict_dir>/reader-dict/<lang>/`.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(database), fields(dict_dir = %dict_dir.display())))]
    pub fn new(database: &Database, dict_dir: &Path) -> Result<Self, MonolingualError> {
        let client = MonolingualClient::new()?;
        let db = Db::new(database);
        Ok(Self {
            client,
            db,
            dict_dir: dict_dir.to_path_buf(),
            pending_installs: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    /// Returns all dictionaries available for download from the remote API.
    ///
    /// Metadata is served from the SQLite cache when available; otherwise a
    /// network request is made and the result is cached.
    ///
    /// # Errors
    ///
    /// Returns an error if the metadata cannot be loaded from cache or network.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn get_available_dictionaries(
        &self,
    ) -> Result<Vec<(String, DictionaryEntry)>, MonolingualError> {
        let metadata = self.load_metadata()?;

        let monolingual = metadata
            .into_iter()
            .filter_map(|(lang, mut targets)| targets.remove(&lang).map(|entry| (lang, entry)))
            .collect();

        Ok(monolingual)
    }

    /// Returns the cached metadata entry for a single language.
    ///
    /// This does not make any network requests. Returns `None` if no entry for
    /// `lang` has been cached yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the database read fails.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(lang = %lang)))]
    pub fn get_entry_for_lang(
        &self,
        lang: &str,
    ) -> Result<Option<DictionaryEntry>, MonolingualError> {
        Ok(self.db.get_entry(lang)?)
    }

    /// Returns the language codes of all locally installed dictionaries.
    ///
    /// A dictionary is considered installed when its language directory exists
    /// inside `<dict_dir>/reader-dict/` and contains at least one `.index`
    /// file paired with a `.dict` or `.dict.dz` file.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be read.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn get_installed_dictionaries(&self) -> Result<Vec<String>, MonolingualError> {
        let root = self.reader_dict_dir();

        if !root.exists() {
            return Ok(Vec::new());
        }

        let mut installed = Vec::new();

        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            if has_dict_pair(&path) {
                if let Some(lang) = path.file_name().and_then(|n| n.to_str()) {
                    installed.push(lang.to_string());
                }
            }
        }

        Ok(installed)
    }

    /// Returns `true` if a download is already in progress for `lang`.
    ///
    /// This can be used by callers to suppress duplicate install requests before
    /// spawning a background thread.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    pub fn is_installing(&self, lang: &str) -> bool {
        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!("lock").entered();
        self.pending_installs().contains(lang)
    }

    fn pending_installs(&self) -> MutexGuard<'_, HashSet<String>> {
        self.pending_installs.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("Pending installs lock poisoned; continuing anyway");
            poisoned.into_inner()
        })
    }

    /// Reserves a language for installation before work moves to a background thread.
    ///
    /// Returns `false` when another install for the same language is already active.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    pub(crate) fn try_begin_install(&self, lang: &str) -> bool {
        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!("lock").entered();

        let mut pending = self.pending_installs();

        if pending.contains(lang) {
            return false;
        }

        pending.insert(lang.to_string());
        true
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    fn finish_install(&self, lang: &str) {
        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!("lock").entered();
        self.pending_installs().remove(lang);
    }

    /// Downloads and installs a dictionary for the given language.
    ///
    /// The archive is downloaded to a temporary file, then extracted into
    /// `<dict_dir>/reader-dict/<lang>/` and the files are renamed to
    /// `Reader-Dict-<lang>.index` and `Reader-Dict-<lang>.dict[.dz]`. Any
    /// existing files in that directory are overwritten.
    ///
    /// Returns [`MonolingualError::InstallationInProgress`] immediately if a
    /// download for the same language is already running. Callers that need to
    /// update UI state before spawning a thread can reserve the language with
    /// [`Self::try_begin_install`] and finish with [`Self::install_reserved_dictionary`].
    ///
    /// # Arguments
    ///
    /// * `entry` - Metadata entry for the dictionary to install. The language
    ///   code and version are derived from this entry.
    /// * `include_etymologies` - When `true`, the full archive (with
    ///   etymologies) is downloaded; when `false`, the smaller no-etymology
    ///   variant is used.
    /// * `progress_callback` - Called after each downloaded chunk with
    ///   `(bytes_downloaded_so_far, total_bytes)`.
    ///
    /// # Errors
    ///
    /// Returns an error if a download for the language is already in progress,
    /// if the download fails, if the archive cannot be parsed, or if files
    /// cannot be written to disk.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, entry, progress_callback), fields(lang = %lang, include_etymologies)))]
    pub fn install_dictionary<F>(
        &self,
        lang: &str,
        entry: &DictionaryEntry,
        include_etymologies: bool,
        progress_callback: &mut F,
    ) -> Result<(), MonolingualError>
    where
        F: FnMut(u64, u64),
    {
        if !self.try_begin_install(lang) {
            return Err(MonolingualError::InstallationInProgress(lang.to_string()));
        }

        self.install_reserved_dictionary(lang, entry, include_etymologies, progress_callback)
    }

    /// Installs a dictionary after [`Self::try_begin_install`] reserves its language.
    ///
    /// This keeps UI state synchronous with background work by allowing callers to
    /// mark a language as installing before spawning a thread.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, entry, progress_callback), fields(lang = %lang, include_etymologies)))]
    pub(crate) fn install_reserved_dictionary<F>(
        &self,
        lang: &str,
        entry: &DictionaryEntry,
        include_etymologies: bool,
        progress_callback: &mut F,
    ) -> Result<(), MonolingualError>
    where
        F: FnMut(u64, u64),
    {
        let result = self.do_install(lang, entry, include_etymologies, progress_callback);
        self.finish_install(lang);

        result
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, entry, progress_callback))
    )]
    fn do_install<F>(
        &self,
        lang: &str,
        entry: &DictionaryEntry,
        include_etymologies: bool,
        progress_callback: &mut F,
    ) -> Result<(), MonolingualError>
    where
        F: FnMut(u64, u64),
    {
        let url = if include_etymologies {
            download_url(lang)
        } else {
            download_url_no_etym(lang)
        };

        tracing::info!(lang, url = %url, "Downloading dictionary");

        let dest = self.lang_dir(lang);
        fs::create_dir_all(&dest)?;

        let temp_path = dest.join(".download.tmp");

        self.client.download(&url, &temp_path, progress_callback)?;

        tracing::debug!(lang, dest = %dest.display(), "Extracting dictionary archive");

        let file = fs::File::open(&temp_path)?;
        extract_zip_renamed(file, &dest, lang)?;

        fs::remove_file(&temp_path)?;

        if let Err(e) = self.db.record_install(lang, entry.updated.into()) {
            tracing::warn!(lang, error = %e, "Failed to record dictionary install");
        }

        tracing::info!(lang, dest = %dest.display(), "Dictionary installed");

        Ok(())
    }

    /// Removes the installed dictionary record for `lang` from the database.
    ///
    /// Logs a warning on failure rather than propagating the error, as this is
    /// a best-effort cleanup step called from event handlers.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn remove_installed(&self, lang: &str) {
        if let Err(e) = self.db.remove_installed(lang) {
            tracing::warn!(lang, error = %e, "Failed to remove installed dictionary record");
        }
    }

    /// Returns `true` if a newer version of the dictionary for `lang` is
    /// available on the server than the currently installed version.
    ///
    /// Returns `false` on any error to avoid surfacing spurious update badges.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn is_update_available(&self, lang: &str) -> bool {
        self.db.is_update_available(lang).unwrap_or(false)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    fn load_metadata(&self) -> Result<DictionariesResponse, MonolingualError> {
        if let Some(cached_at) = self.db.get_most_recent_cached_at()? {
            match self.client.is_metadata_modified_since(cached_at) {
                Ok(false) => {
                    tracing::debug!("Cache is fresh (304), using cached metadata");
                    if let Some(cached) = self.get_cached_metadata()? {
                        return Ok(cached);
                    }
                }
                Ok(true) => {
                    tracing::debug!("API has newer data (200), refreshing cache");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "HEAD check failed, falling back to cache");
                    if let Some(cached) = self.get_cached_metadata()? {
                        return Ok(cached);
                    }
                }
            }
        }

        self.fetch_and_cache_metadata().or_else(|_| {
            self.get_cached_metadata()?
                .ok_or_else(|| MonolingualError::NotFound("metadata unavailable".to_string()))
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    fn fetch_and_cache_metadata(&self) -> Result<DictionariesResponse, MonolingualError> {
        let metadata = self.client.fetch_metadata()?;

        for (source_lang, targets) in &metadata {
            if let Some(entry) = targets.get(source_lang.as_str()) {
                self.db.upsert_entry(source_lang, entry)?;
            }
        }

        tracing::debug!("Cached monolingual metadata to database");
        Ok(metadata)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    fn get_cached_metadata(&self) -> Result<Option<DictionariesResponse>, MonolingualError> {
        let entries = self.db.get_all_entries()?;

        if entries.is_empty() {
            tracing::debug!("No cached metadata found in database");
            return Ok(None);
        }

        let mut response = DictionariesResponse::new();
        for (lang, entry) in entries {
            response
                .entry(lang.clone())
                .or_default()
                .insert(lang, entry);
        }

        tracing::debug!("Loaded cached metadata from database");
        Ok(Some(response))
    }

    fn reader_dict_dir(&self) -> PathBuf {
        self.dict_dir.join(READER_DICT_SUBDIR)
    }

    fn lang_dir(&self, lang: &str) -> PathBuf {
        self.reader_dict_dir().join(lang)
    }
}

/// Returns `true` when `dir` contains at least one `.index` file that is
/// paired with a `.dict` or `.dict.dz` file sharing the same stem.
fn has_dict_pair(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if !name.ends_with(".index") {
            continue;
        }

        let stem = &name[..name.len() - ".index".len()];
        let dict = dir.join(format!("{stem}.dict"));
        let dict_dz = dir.join(format!("{stem}.dict.dz"));

        if dict.exists() || dict_dz.exists() {
            return true;
        }
    }

    false
}

/// Extracts all entries from a ZIP archive into `dest`, renaming each
/// file to `Reader-Dict-<lang><ext>` where `<ext>` is `.index`, `.dict`,
/// or `.dict.dz`.
///
/// Files with unrecognised extensions are skipped. Directories inside the ZIP
/// are ignored because all output files land flat in `dest`.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(reader)))]
fn extract_zip_renamed<R: std::io::Read + std::io::Seek>(
    reader: R,
    dest: &Path,
    lang: &str,
) -> Result<(), MonolingualError> {
    let mut archive = ZipArchive::new(reader)
        .map_err(|e| MonolingualError::Extraction(format!("failed to open zip archive: {e}")))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| {
            MonolingualError::Extraction(format!("failed to read zip entry {i}: {e}"))
        })?;

        if file.is_dir() {
            continue;
        }

        let original_name = match file.enclosed_name() {
            Some(p) => p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string(),
            None => {
                tracing::warn!(index = i, "Skipping zip entry with unsafe path");
                continue;
            }
        };

        let target_name = dict_file_target_name(&original_name, lang);
        let Some(target_name) = target_name else {
            tracing::debug!(
                original_name,
                "Skipping zip entry with unrecognised extension"
            );
            continue;
        };

        let out_path = dest.join(&target_name);
        let mut out_file = fs::File::create(&out_path)?;
        io::copy(&mut file, &mut out_file)?;
        tracing::debug!(path = %out_path.display(), "Extracted file");
    }

    Ok(())
}

/// Maps a ZIP entry filename to its renamed output filename `<lang>.<ext>`.
///
/// Recognised extensions (in priority order):
/// - `.dict.dz` → `Reader-Dict-<lang>.dict.dz`
/// - `.dict`    → `Reader-Dict-<lang>.dict`
/// - `.index`   → `Reader-Dict-<lang>.index`
///
/// Returns `None` for any other extension.
fn dict_file_target_name(original: &str, lang: &str) -> Option<String> {
    for ext in &[".dict.dz", ".dict", ".index"] {
        if original.ends_with(ext) {
            return Some(format!("Reader-Dict-{lang}{ext}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::dictionary::monolingual::metadata::DictionaryEntry;
    use chrono::NaiveDate;
    use std::io::Cursor;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_service() -> (MonolingualDictionaryService, TempDir, Database) {
        crate::crypto::init_crypto_provider();
        let dir = TempDir::new().expect("failed to create temp dir");
        let mut database = Database::new(":memory:").expect("failed to create in-memory database");
        database.init(0).expect("failed to run migrations");
        let service = MonolingualDictionaryService::new(&database, dir.path())
            .expect("failed to create service");
        (service, dir, database)
    }

    fn make_entry(year: i32, month: u32, day: u32) -> DictionaryEntry {
        DictionaryEntry {
            formats: "df,dic,dictorg,kobo,mobi,stardict".to_string(),
            updated: NaiveDate::from_ymd_opt(year, month, day).unwrap(),
            words: 1_381_375,
        }
    }

    #[test]
    fn test_get_installed_empty_when_no_dir() {
        let (service, _dir, _db) = create_test_service();
        let installed = service.get_installed_dictionaries().unwrap();
        assert!(installed.is_empty());
    }

    #[test]
    fn test_get_installed_empty_when_dir_exists_but_empty() {
        let (service, dir, _db) = create_test_service();
        fs::create_dir_all(dir.path().join(READER_DICT_SUBDIR)).unwrap();
        let installed = service.get_installed_dictionaries().unwrap();
        assert!(installed.is_empty());
    }

    #[test]
    fn test_get_installed_detects_dict_pair() {
        let (service, dir, _db) = create_test_service();
        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("en");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();
        fs::File::create(lang_dir.join("dict.dict")).unwrap();

        let installed = service.get_installed_dictionaries().unwrap();
        assert_eq!(installed, vec!["en".to_string()]);
    }

    #[test]
    fn test_get_installed_detects_dict_dz_pair() {
        let (service, dir, _db) = create_test_service();
        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("fr");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();
        fs::File::create(lang_dir.join("dict.dict.dz")).unwrap();

        let installed = service.get_installed_dictionaries().unwrap();
        assert_eq!(installed, vec!["fr".to_string()]);
    }

    #[test]
    fn test_get_installed_ignores_index_without_dict() {
        let (service, dir, _db) = create_test_service();
        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("de");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();

        let installed = service.get_installed_dictionaries().unwrap();
        assert!(installed.is_empty());
    }

    #[test]
    fn test_install_dictionary_extracts_zip_renamed() {
        let (_service, dir, _db) = create_test_service();

        let zip_bytes = make_test_zip(&[
            ("dictorg-en-en.index", b"index content"),
            ("dictorg-en-en.dict", b"dict content"),
        ]);

        let dest = dir.path().join(READER_DICT_SUBDIR).join("en");
        fs::create_dir_all(&dest).unwrap();
        extract_zip_renamed(Cursor::new(&zip_bytes), &dest, "en").unwrap();

        assert!(dest.join("Reader-Dict-en.index").exists());
        assert!(dest.join("Reader-Dict-en.dict").exists());
    }

    fn make_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default();
            for (name, content) in entries {
                zip.start_file(*name, options).unwrap();
                zip.write_all(content).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn test_is_installing_false_initially() {
        let (service, _dir, _db) = create_test_service();
        assert!(!service.is_installing("en"));
    }

    #[test]
    fn test_is_installing_true_while_pending() {
        let (service, _dir, _db) = create_test_service();
        service
            .pending_installs
            .lock()
            .unwrap()
            .insert("fr".to_string());
        assert!(service.is_installing("fr"));
        assert!(!service.is_installing("en"));
    }

    #[test]
    fn test_try_begin_install_marks_pending_and_blocks_duplicate() {
        let (service, _dir, _db) = create_test_service();

        assert!(service.try_begin_install("en"));
        assert!(service.is_installing("en"));
        assert!(!service.try_begin_install("en"));
    }

    #[test]
    fn test_pending_installs_recovers_from_poisoned_lock() {
        let (service, _dir, _db) = create_test_service();
        let service_clone = service.clone();

        let result = std::thread::spawn(move || {
            let _guard = service_clone.pending_installs.lock().unwrap();
            panic!("poison pending installs lock");
        })
        .join();

        assert!(result.is_err());
        assert!(service.try_begin_install("en"));
        assert!(service.is_installing("en"));
        service.finish_install("en");
        assert!(!service.is_installing("en"));
    }

    #[test]
    fn test_is_installing_false_after_removal() {
        let (service, _dir, _db) = create_test_service();
        service
            .pending_installs
            .lock()
            .unwrap()
            .insert("en".to_string());
        service.pending_installs.lock().unwrap().remove("en");
        assert!(!service.is_installing("en"));
    }

    #[test]
    fn test_concurrent_install_same_lang_returns_error() {
        let (service, _dir, _db) = create_test_service();
        service
            .pending_installs
            .lock()
            .unwrap()
            .insert("de".to_string());

        let entry = make_entry(2026, 4, 1);
        let err = service
            .install_dictionary("de", &entry, false, &mut |_, _| {})
            .expect_err("expected InstallationInProgress error");

        assert!(
            matches!(err, MonolingualError::InstallationInProgress(_)),
            "unexpected error variant: {err}"
        );
    }

    #[test]
    fn test_pending_cleared_after_failed_install() {
        let (service, _dir, _db) = create_test_service();

        let entry = make_entry(2026, 4, 1);
        let _ = service.install_dictionary("zz", &entry, false, &mut |_, _| {});
        assert!(!service.is_installing("zz"));
    }

    #[test]
    fn test_reserved_install_clears_after_failed_install() {
        let (service, _dir, _db) = create_test_service();
        let entry = make_entry(2026, 4, 1);

        assert!(service.try_begin_install("zz"));

        let _ = service.install_reserved_dictionary("zz", &entry, false, &mut |_, _| {});

        assert!(!service.is_installing("zz"));
    }

    #[test]
    fn test_is_installing_shared_across_clones() {
        let (service, _dir, _db) = create_test_service();
        let clone = service.clone();

        service
            .pending_installs
            .lock()
            .unwrap()
            .insert("ja".to_string());

        assert!(clone.is_installing("ja"));
    }

    #[test]
    fn test_get_entry_for_lang_returns_none_when_not_cached() {
        let (service, _dir, _db) = create_test_service();
        let result = service.get_entry_for_lang("en").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_entry_for_lang_returns_entry_after_cache() {
        let (service, _dir, _db) = create_test_service();

        let entry = make_entry(2026, 4, 1);
        service.db.upsert_entry("en", &entry).unwrap();

        let result = service.get_entry_for_lang("en").unwrap();
        assert!(result.is_some());
        let fetched = result.unwrap();
        assert_eq!(fetched.words, 1_381_375);
        assert_eq!(
            fetched.updated,
            NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
        );
    }

    /// Downloads and installs the English dictionary from the live API, then
    /// verifies that at least one `.index` + `.dict`/`.dict.dz` pair is present.
    ///
    /// Run with: `cargo test -- --ignored`
    #[test]
    #[ignore = "requires network access to www.reader-dict.com"]
    fn test_install_dictionary_live() {
        let (service, dir, _db) = create_test_service();

        let entry = service
            .get_available_dictionaries()
            .unwrap()
            .into_iter()
            .find(|(l, _)| l == "en")
            .map(|(_, e)| e)
            .expect("English dictionary should be available");

        service
            .install_dictionary("en", &entry, false, &mut |_, _| {})
            .expect("install_dictionary failed");

        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("en");
        assert!(
            lang_dir.exists(),
            "language directory should exist after install"
        );
        assert!(
            has_dict_pair(&lang_dir),
            "expected .index + .dict/.dict.dz pair in {lang_dir:?}"
        );

        let installed = service
            .get_installed_dictionaries()
            .expect("get_installed_dictionaries failed");
        assert!(
            installed.contains(&"en".to_string()),
            "expected 'en' in installed list, got {installed:?}"
        );
    }
}
