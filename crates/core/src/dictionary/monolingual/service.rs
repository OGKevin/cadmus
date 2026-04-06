//! High-level service for monolingual dictionary management.
//!
//! [`MonolingualDictionaryService`] is the single public entry point for all
//! monolingual dictionary operations: querying the remote catalogue, listing
//! installed dictionaries, and installing a new one.

use super::client::MonolingualClient;
use super::errors::MonolingualError;
use super::metadata::{download_url, download_url_no_etym, DictionaryEntry};
use crate::db::Database;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    #[cfg_attr(feature = "otel", tracing::instrument(skip(database), fields(dict_dir = %dict_dir.display())))]
    pub fn new(database: &Database, dict_dir: &Path) -> Result<Self, MonolingualError> {
        let client = MonolingualClient::new(database)?;
        Ok(Self {
            client,
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
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub fn get_available_dictionaries(
        &self,
    ) -> Result<Vec<(String, DictionaryEntry)>, MonolingualError> {
        self.client.get_available_dictionaries()
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
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
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
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    pub fn is_installing(&self, lang: &str) -> bool {
        #[cfg(feature = "otel")]
        let _span = tracing::info_span!("lock").entered();
        self.pending_installs.lock().unwrap().contains(lang)
    }

    /// Downloads and installs a dictionary for the given language.
    ///
    /// The archive is extracted into `<dict_dir>/reader-dict/<lang>/` and the
    /// files are renamed to `<lang>.index` and `<lang>.dict[.dz]`. Any
    /// existing files in that directory are overwritten.
    ///
    /// Returns [`MonolingualError::InstallationInProgress`] immediately if a
    /// download for the same language is already running. The caller should
    /// check [`Self::is_installing`] on the UI thread before spawning a thread
    /// to get a user-visible early exit; this check inside `install_dictionary`
    /// provides a safety net against races.
    ///
    /// # Arguments
    ///
    /// * `lang` - ISO 639-1 language code (e.g. `"en"`, `"fr"`).
    /// * `include_etymologies` - When `true`, the full archive (with
    ///   etymologies) is downloaded; when `false`, the smaller no-etymology
    ///   variant is used.
    ///
    /// # Errors
    ///
    /// Returns an error if a download for `lang` is already in progress, if
    /// the download fails, if the archive cannot be parsed, or if files cannot
    /// be written to disk.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(lang = %lang, include_etymologies)))]
    pub fn install_dictionary(
        &self,
        lang: &str,
        include_etymologies: bool,
    ) -> Result<(), MonolingualError> {
        {
            #[cfg(feature = "otel")]
            let _span = tracing::info_span!("lock").entered();

            let mut pending = self.pending_installs.lock().unwrap();
            if pending.contains(lang) {
                return Err(MonolingualError::InstallationInProgress(lang.to_string()));
            }
            pending.insert(lang.to_string());
        }

        let result = self.do_install(lang, include_etymologies);

        {
            #[cfg(feature = "otel")]
            let _span = tracing::info_span!("lock").entered();
            self.pending_installs.lock().unwrap().remove(lang);
        }

        result
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    fn do_install(&self, lang: &str, include_etymologies: bool) -> Result<(), MonolingualError> {
        let url = if include_etymologies {
            download_url(lang)
        } else {
            download_url_no_etym(lang)
        };

        tracing::info!(lang, url = %url, "Downloading dictionary");

        let bytes = self
            .client
            .download_bytes(&url)
            .map_err(|e| MonolingualError::Request(e.to_string()))?;

        let dest = self.lang_dir(lang);
        fs::create_dir_all(&dest)?;

        tracing::debug!(lang, dest = %dest.display(), "Extracting dictionary archive");

        extract_zip_renamed(&bytes, &dest, lang)?;

        tracing::info!(lang, dest = %dest.display(), "Dictionary installed");

        Ok(())
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

/// Extracts all entries from a ZIP archive `bytes` into `dest`, renaming each
/// file to `<lang>.<ext>` where `<ext>` is `.index`, `.dict`, or `.dict.dz`.
///
/// Files with unrecognised extensions are skipped. Directories inside the ZIP
/// are ignored because all output files land flat in `dest`.
#[cfg_attr(feature = "otel", tracing::instrument(skip(bytes)))]
fn extract_zip_renamed(bytes: &[u8], dest: &Path, lang: &str) -> Result<(), MonolingualError> {
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor)
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
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_service() -> (MonolingualDictionaryService, TempDir) {
        crate::crypto::init_crypto_provider();
        let dir = TempDir::new().expect("failed to create temp dir");
        let database = Database::new(":memory:").expect("failed to create in-memory database");
        database.migrate().expect("failed to run migrations");
        let service = MonolingualDictionaryService::new(&database, dir.path())
            .expect("failed to create service");
        (service, dir)
    }

    #[test]
    fn test_get_installed_empty_when_no_dir() {
        let (service, _dir) = create_test_service();
        let installed = service.get_installed_dictionaries().unwrap();
        assert!(installed.is_empty());
    }

    #[test]
    fn test_get_installed_empty_when_dir_exists_but_empty() {
        let (service, dir) = create_test_service();
        fs::create_dir_all(dir.path().join(READER_DICT_SUBDIR)).unwrap();
        let installed = service.get_installed_dictionaries().unwrap();
        assert!(installed.is_empty());
    }

    #[test]
    fn test_get_installed_detects_dict_pair() {
        let (service, dir) = create_test_service();
        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("en");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();
        fs::File::create(lang_dir.join("dict.dict")).unwrap();

        let installed = service.get_installed_dictionaries().unwrap();
        assert_eq!(installed, vec!["en".to_string()]);
    }

    #[test]
    fn test_get_installed_detects_dict_dz_pair() {
        let (service, dir) = create_test_service();
        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("fr");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();
        fs::File::create(lang_dir.join("dict.dict.dz")).unwrap();

        let installed = service.get_installed_dictionaries().unwrap();
        assert_eq!(installed, vec!["fr".to_string()]);
    }

    #[test]
    fn test_get_installed_ignores_index_without_dict() {
        let (service, dir) = create_test_service();
        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("de");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();

        let installed = service.get_installed_dictionaries().unwrap();
        assert!(installed.is_empty());
    }

    #[test]
    fn test_install_dictionary_extracts_zip_renamed() {
        let (_service, dir) = create_test_service();

        let zip_bytes = make_test_zip(&[
            ("dictorg-en-en.index", b"index content"),
            ("dictorg-en-en.dict", b"dict content"),
        ]);

        let dest = dir.path().join(READER_DICT_SUBDIR).join("en");
        fs::create_dir_all(&dest).unwrap();
        extract_zip_renamed(&zip_bytes, &dest, "en").unwrap();

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
        let (service, _dir) = create_test_service();
        assert!(!service.is_installing("en"));
    }

    #[test]
    fn test_is_installing_true_while_pending() {
        let (service, _dir) = create_test_service();
        service
            .pending_installs
            .lock()
            .unwrap()
            .insert("fr".to_string());
        assert!(service.is_installing("fr"));
        assert!(!service.is_installing("en"));
    }

    #[test]
    fn test_is_installing_false_after_removal() {
        let (service, _dir) = create_test_service();
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
        let (service, _dir) = create_test_service();
        service
            .pending_installs
            .lock()
            .unwrap()
            .insert("de".to_string());

        let err = service
            .install_dictionary("de", false)
            .expect_err("expected InstallationInProgress error");

        assert!(
            matches!(err, MonolingualError::InstallationInProgress(_)),
            "unexpected error variant: {err}"
        );
    }

    #[test]
    fn test_pending_cleared_after_failed_install() {
        let (service, _dir) = create_test_service();

        // A missing URL will cause a network error; verify pending is cleared.
        let _ = service.install_dictionary("zz", false);
        assert!(!service.is_installing("zz"));
    }

    #[test]
    fn test_is_installing_shared_across_clones() {
        let (service, _dir) = create_test_service();
        let clone = service.clone();

        service
            .pending_installs
            .lock()
            .unwrap()
            .insert("ja".to_string());

        assert!(clone.is_installing("ja"));
    }

    /// Downloads and installs the English dictionary from the live API, then
    /// verifies that at least one `.index` + `.dict`/`.dict.dz` pair is present.
    ///
    /// Run with: `cargo test -- --ignored`
    #[test]
    #[ignore = "requires network access to www.reader-dict.com"]
    fn test_install_dictionary_live() {
        let (service, dir) = create_test_service();

        service
            .install_dictionary("en", false)
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
