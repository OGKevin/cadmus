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
use crate::db::types::UnixTimestamp;
use std::collections::HashSet;
use std::fs;
use std::io::{self};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use zip::ZipArchive;

/// Subdirectory inside the dictionaries root where reader-dict downloads live.
const READER_DICT_SUBDIR: &str = "reader-dict";
const STAGING_SUFFIX: &str = ".partial";
const REPLACED_SUFFIX: &str = ".replaced";
const DOWNLOAD_TMP_NAME: &str = ".download.tmp";

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
    /// A dictionary is listed only when it is recorded in the registry and its
    /// language directory contains a complete `.index` + `.dict`/`.dict.dz` pair.
    ///
    /// # Errors
    ///
    /// Returns an error if the registry cannot be read.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn get_installed_dictionaries(&self) -> Result<Vec<String>, MonolingualError> {
        let registered = self.db.list_installed_langs()?;
        Ok(registered
            .into_iter()
            .filter(|lang| has_dict_pair(&self.lang_dir(lang)))
            .collect())
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
    pub(crate) fn finish_install(&self, lang: &str) {
        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!("lock").entered();
        self.pending_installs().remove(lang);
    }

    /// Downloads and installs a dictionary for the given language.
    ///
    /// The archive is downloaded into a hidden staging directory, extracted
    /// there, then moved to `<dict_dir>/reader-dict/<lang>/` with a single
    /// rename. Files are named `Reader-Dict-<lang>.index` and
    /// `Reader-Dict-<lang>.dict[.dz]`. An existing language directory is
    /// moved aside before that rename and restored if publishing or registry
    /// recording fails. Installation is recorded in the registry only after
    /// the dest exists; the aside is removed only after that write succeeds.
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
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, entry, progress_callback), fields(lang = %lang, include_etymologies = include_etymologies)))]
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
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, entry, progress_callback), fields(lang = %lang, include_etymologies = include_etymologies)))]
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
        tracing::instrument(skip(self, entry, progress_callback), fields(lang = %lang, include_etymologies = include_etymologies))
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
        let staging = staging_dir(&self.reader_dict_dir(), lang);
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(&staging)?;
        let mut staging_guard = crate::fs::RemovePathOnDrop::dir(staging.clone());
        let temp_path = staging.join(DOWNLOAD_TMP_NAME);

        self.client.download(&url, &temp_path, progress_callback)?;

        tracing::debug!(lang, dest = %dest.display(), "Extracting dictionary archive");

        let file = fs::File::open(&temp_path)?;
        extract_zip_renamed(file, &staging, lang)?;
        if let Err(error) = fs::remove_file(&temp_path) {
            tracing::warn!(
                path = %temp_path.display(),
                error = %error,
                "failed to remove dictionary download temp file"
            );
        }

        let previous = commit_extracted_dictionary(&dest, &staging, lang, &mut staging_guard)?;
        if let Err(registry) = self.db.record_install(lang, entry.updated.into()) {
            if let Err(restore) =
                restore_previous_dictionary_after_registry_failure(&dest, previous.as_ref())
            {
                return Err(MonolingualError::InstallRecordAndRestore {
                    registry: registry.to_string(),
                    restore: restore.to_string(),
                });
            }
            return Err(registry.into());
        }
        discard_replaced_aside(previous.as_ref());

        tracing::info!(lang, dest = %dest.display(), "Dictionary installed");

        Ok(())
    }

    /// Removes the installed dictionary record for `lang` and leftover install trees.
    ///
    /// Also deletes `.{lang}.replaced`, `.{lang}.partial`, and the language
    /// destination, in that order, so startup reconcile cannot restore a
    /// dictionary the user just removed. Logs a warning on failure rather than
    /// propagating the error, as this is a best-effort cleanup step called from
    /// event handlers.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn remove_installed(&self, lang: &str) {
        let root = self.reader_dict_dir();
        for path in [
            replaced_dir(&root, lang),
            staging_dir(&root, lang),
            self.lang_dir(lang),
        ] {
            if path.exists()
                && let Err(error) = fs::remove_dir_all(&path)
            {
                tracing::warn!(
                    lang,
                    path = %path.display(),
                    error = %error,
                    "Failed to remove leftover dictionary install directory"
                );
            }
        }
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

    #[cfg(test)]
    fn reconcile(&self) -> Result<(), MonolingualError> {
        reconcile_reader_dict_tree(&self.db, &self.reader_dict_dir())
    }
}

/// Registers complete disk installs and removes abandoned staging directories.
///
/// Cadmus automatically indexes downloads, updates, and re-downloads. Manually
/// copied dictionaries require restarting Cadmus before they appear in the
/// installed list. Invoked at process startup so a hand-placed complete
/// dictionary becomes listed, and leftover `.lang.partial` trees from interrupted
/// installs go away.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(database), fields(dict_dir = %dict_dir.display()))
)]
pub(crate) fn reconcile_installed_dictionaries(
    database: &Database,
    dict_dir: &Path,
) -> Result<(), MonolingualError> {
    let db = Db::new(database);
    reconcile_reader_dict_tree(&db, &dict_dir.join(READER_DICT_SUBDIR))
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(db), fields(root = %root.display()))
)]
fn reconcile_reader_dict_tree(db: &Db, root: &Path) -> Result<(), MonolingualError> {
    if !root.exists() {
        tracing::debug!(root = %root.display(), "no reader-dict directory to reconcile");
        return Ok(());
    }

    let mut registered: HashSet<String> = db.list_installed_langs()?.into_iter().collect();
    tracing::debug!(
        registered = registered.len(),
        "reconciling dictionary installs"
    );

    let mut stagings = Vec::new();
    let mut replaced = Vec::new();
    let mut dests = Vec::new();

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(ToOwned::to_owned)
        else {
            continue;
        };

        if let Some(lang) = staging_lang(&name).map(str::to_owned) {
            stagings.push((path, lang));
            continue;
        }
        if let Some(lang) = replaced_lang(&name).map(str::to_owned) {
            replaced.push((path, lang));
            continue;
        }
        if !name.starts_with('.') && path.is_dir() {
            dests.push((path, name));
        }
    }

    let mut failures = Vec::new();
    for (path, lang) in stagings {
        if let Err(error) = reconcile_staging_dir(db, root, &path, &lang, &mut registered) {
            tracing::warn!(
                lang,
                path = %path.display(),
                error = %error,
                "failed to reconcile dictionary staging directory"
            );
            failures.push((lang, error));
        }
    }
    for (path, lang) in replaced {
        if let Err(error) = reconcile_replaced_dir(root, &path, &lang, &registered) {
            tracing::warn!(
                lang,
                path = %path.display(),
                error = %error,
                "failed to reconcile replaced dictionary directory"
            );
            failures.push((lang, error));
        }
    }
    for (path, name) in dests {
        if has_dict_pair(&path)
            && registered.insert(name.clone())
            && let Err(error) = register_complete_install(db, &name)
        {
            registered.remove(&name);
            tracing::warn!(
                lang = name.as_str(),
                path = %path.display(),
                error = %error,
                "failed to register complete dictionary install"
            );
            failures.push((name, error));
        }
    }

    if !failures.is_empty() {
        let count = failures.len();
        let summary = failures
            .iter()
            .map(|(lang, error)| format!("{lang}: {error}"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(MonolingualError::ReconcileIncomplete { count, summary });
    }

    tracing::debug!("dictionary install reconciliation complete");
    Ok(())
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(db, root, registered), fields(lang = %lang, staging = %staging.display()))
)]
fn reconcile_staging_dir(
    db: &Db,
    root: &Path,
    staging: &Path,
    lang: &str,
    registered: &mut HashSet<String>,
) -> Result<(), MonolingualError> {
    let dest = root.join(lang);
    if has_dict_pair(staging) && !has_dict_pair(&dest) {
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
            tracing::info!(
                lang,
                dest = %dest.display(),
                "removed incomplete dictionary dest before promoting staging"
            );
        }
        fs::rename(staging, &dest)?;
        tracing::info!(
            lang,
            dest = %dest.display(),
            "promoted complete dictionary staging"
        );
        if !registered.contains(lang) {
            register_complete_install(db, lang)?;
            registered.insert(lang.to_owned());
        }
        return Ok(());
    }

    if let Err(error) = fs::remove_dir_all(staging) {
        tracing::warn!(
            path = %staging.display(),
            error = %error,
            "failed to remove abandoned dictionary staging directory"
        );
    } else {
        tracing::debug!(
            lang,
            path = %staging.display(),
            "removed abandoned dictionary staging"
        );
    }
    Ok(())
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(root, registered), fields(lang = %lang, replaced = %replaced.display()))
)]
fn reconcile_replaced_dir(
    root: &Path,
    replaced: &Path,
    lang: &str,
    registered: &HashSet<String>,
) -> Result<(), MonolingualError> {
    let dest = root.join(lang);
    let dest_complete = has_dict_pair(&dest);
    let aside_complete = has_dict_pair(replaced);
    let is_registered = registered.contains(lang);

    if dest_complete {
        discard_replaced_tree(replaced, lang);
        return Ok(());
    }

    if aside_complete && is_registered {
        restore_replaced_to_dest(&dest, replaced, lang)?;
        return Ok(());
    }

    discard_replaced_tree(replaced, lang);
    Ok(())
}

fn restore_replaced_to_dest(
    dest: &Path,
    replaced: &Path,
    lang: &str,
) -> Result<(), MonolingualError> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
        tracing::info!(
            lang,
            dest = %dest.display(),
            "removed dictionary dest before restoring replaced"
        );
    }
    fs::rename(replaced, dest)?;
    tracing::info!(
        lang,
        dest = %dest.display(),
        "restored replaced dictionary directory"
    );
    Ok(())
}

fn discard_replaced_tree(replaced: &Path, lang: &str) {
    if let Err(error) = fs::remove_dir_all(replaced) {
        tracing::warn!(
            path = %replaced.display(),
            error = %error,
            "failed to remove leftover replaced dictionary directory"
        );
    } else {
        tracing::debug!(
            lang,
            path = %replaced.display(),
            "removed leftover replaced dictionary directory"
        );
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(db), fields(lang = %lang)))]
fn register_complete_install(db: &Db, lang: &str) -> Result<(), MonolingualError> {
    let version = install_version_for_registration(db, lang)?;
    db.record_install(lang, version)?;
    tracing::info!(lang, "registered complete dictionary install");
    Ok(())
}

/// Version to record when registering a complete on-disk install.
///
/// Uses the cached catalogue `updated` date when known. Otherwise records the
/// Unix epoch so any later published build is treated as newer.
fn install_version_for_registration(
    db: &Db,
    lang: &str,
) -> Result<UnixTimestamp, MonolingualError> {
    Ok(match db.get_entry(lang)? {
        Some(entry) => entry.updated.into(),
        None => UnixTimestamp::from(0),
    })
}

fn staging_dir(root: &Path, lang: &str) -> PathBuf {
    root.join(format!(".{lang}{STAGING_SUFFIX}"))
}

fn replaced_dir(root: &Path, lang: &str) -> PathBuf {
    root.join(format!(".{lang}{REPLACED_SUFFIX}"))
}

fn staging_lang(name: &str) -> Option<&str> {
    name.strip_prefix('.')?
        .strip_suffix(STAGING_SUFFIX)
        .filter(|lang| !lang.is_empty())
}

fn replaced_lang(name: &str) -> Option<&str> {
    name.strip_prefix('.')?
        .strip_suffix(REPLACED_SUFFIX)
        .filter(|lang| !lang.is_empty())
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(staging_guard), fields(lang = %lang, dest = %dest.display(), staging = %staging.display()))
)]
fn commit_extracted_dictionary(
    dest: &Path,
    staging: &Path,
    lang: &str,
    staging_guard: &mut crate::fs::RemovePathOnDrop,
) -> Result<Option<PathBuf>, MonolingualError> {
    if !has_dict_pair(staging) {
        return Err(MonolingualError::Extraction(
            "archive did not contain a complete .index and .dict pair".to_string(),
        ));
    }
    Ok(publish_extracted_dictionary(
        dest,
        staging,
        lang,
        staging_guard,
    )?)
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(staging_guard), fields(lang = %lang, dest = %dest.display(), staging = %staging.display()))
)]
fn publish_extracted_dictionary(
    dest: &Path,
    staging: &Path,
    lang: &str,
    staging_guard: &mut crate::fs::RemovePathOnDrop,
) -> Result<Option<PathBuf>, io::Error> {
    let aside = dest
        .parent()
        .map(|root| replaced_dir(root, lang))
        .unwrap_or_else(|| dest.with_file_name(format!(".{lang}{REPLACED_SUFFIX}")));
    if aside.exists() {
        fs::remove_dir_all(&aside)?;
    }

    let mut restore = None;
    let mut previous = None;
    if dest.exists() {
        fs::rename(dest, &aside)?;
        restore = Some(crate::fs::RestorePathOnDrop::new(
            aside.clone(),
            dest.to_path_buf(),
        ));
        previous = Some(aside.clone());
    }

    fs::rename(staging, dest)?;
    staging_guard.disarm();
    if let Some(mut restore) = restore {
        restore.disarm();
    }
    Ok(previous)
}

fn restore_previous_dictionary_after_registry_failure(
    dest: &Path,
    previous: Option<&PathBuf>,
) -> Result<(), MonolingualError> {
    let Some(aside) = previous else {
        return Ok(());
    };

    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::rename(aside, dest)?;
    tracing::warn!(
        dest = %dest.display(),
        aside = %aside.display(),
        "restored previous dictionary after registry write failed"
    );
    Ok(())
}

fn discard_replaced_aside(previous: Option<&PathBuf>) {
    let Some(aside) = previous else {
        return;
    };

    if let Err(error) = fs::remove_dir_all(aside) {
        tracing::warn!(
            path = %aside.display(),
            error = %error,
            "failed to remove replaced dictionary directory"
        );
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
        database.init_for_test(0).expect("failed to run migrations");
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
        service
            .db
            .record_install("en", UnixTimestamp::now())
            .unwrap();

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
        service
            .db
            .record_install("fr", UnixTimestamp::now())
            .unwrap();

        let installed = service.get_installed_dictionaries().unwrap();
        assert_eq!(installed, vec!["fr".to_string()]);
    }

    #[test]
    fn test_get_installed_ignores_complete_files_without_registry() {
        let (service, dir, _db) = create_test_service();
        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("en");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();
        fs::File::create(lang_dir.join("dict.dict")).unwrap();

        let installed = service.get_installed_dictionaries().unwrap();
        assert!(installed.is_empty());
    }

    #[test]
    fn test_partial_extract_is_not_installed_and_does_not_block_dest() {
        let (service, dir, _db) = create_test_service();
        let staging = staging_dir(&dir.path().join(READER_DICT_SUBDIR), "en");
        fs::create_dir_all(&staging).unwrap();
        fs::File::create(staging.join("dict.index")).unwrap();

        assert!(service.get_installed_dictionaries().unwrap().is_empty());

        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("en");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();
        fs::File::create(lang_dir.join("dict.dict")).unwrap();
        service
            .db
            .record_install("en", UnixTimestamp::now())
            .unwrap();

        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
        assert!(staging.exists());

        service.reconcile().unwrap();
        assert!(
            !staging.exists(),
            "incomplete staging must not block a later install"
        );
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_promotes_complete_staging_over_incomplete_dest() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let staging = staging_dir(&root, "en");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"old").unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("dict.index"), b"new").unwrap();
        fs::write(staging.join("dict.dict"), b"new").unwrap();

        service.reconcile().unwrap();

        assert!(!staging.exists());
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"new");
        assert!(has_dict_pair(&dest));
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_keeps_complete_dest_and_drops_complete_staging() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let staging = staging_dir(&root, "en");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"old").unwrap();
        fs::write(dest.join("dict.dict"), b"old").unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("dict.index"), b"new").unwrap();
        fs::write(staging.join("dict.dict"), b"new").unwrap();
        service
            .db
            .record_install("en", UnixTimestamp::now())
            .unwrap();

        service.reconcile().unwrap();

        assert!(!staging.exists());
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"old");
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_promotes_complete_staging_when_dest_missing() {
        let (service, dir, _db) = create_test_service();
        let staging = staging_dir(&dir.path().join(READER_DICT_SUBDIR), "en");
        fs::create_dir_all(&staging).unwrap();
        fs::File::create(staging.join("dict.index")).unwrap();
        fs::File::create(staging.join("dict.dict")).unwrap();

        assert!(service.get_installed_dictionaries().unwrap().is_empty());
        service.reconcile().unwrap();
        assert!(!staging.exists());
        assert!(has_dict_pair(
            &dir.path().join(READER_DICT_SUBDIR).join("en")
        ));
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_registers_complete_unrecorded_install() {
        let (service, dir, _db) = create_test_service();
        let lang_dir = dir.path().join(READER_DICT_SUBDIR).join("en");
        fs::create_dir_all(&lang_dir).unwrap();
        fs::File::create(lang_dir.join("dict.index")).unwrap();
        fs::File::create(lang_dir.join("dict.dict")).unwrap();

        assert!(service.get_installed_dictionaries().unwrap().is_empty());
        service.reconcile().unwrap();
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_removes_abandoned_temps_without_accumulating() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);

        for _ in 0..2 {
            let staging = staging_dir(&root, "en");
            fs::create_dir_all(&staging).unwrap();
            fs::File::create(staging.join(DOWNLOAD_TMP_NAME)).unwrap();
            fs::File::create(staging.join("dict.index")).unwrap();
            service.reconcile().unwrap();
        }

        let leftover: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(
            leftover.is_empty(),
            "expected no leftover staging or temp files, got {leftover:?}"
        );
        assert!(service.get_installed_dictionaries().unwrap().is_empty());
    }

    #[test]
    fn test_incomplete_extract_does_not_replace_installed_dest() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let staging = staging_dir(&root, "en");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"old").unwrap();
        fs::write(dest.join("dict.dict"), b"old").unwrap();
        service
            .db
            .record_install("en", UnixTimestamp::now())
            .unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("dict.index"), b"new").unwrap();

        let mut guard = crate::fs::RemovePathOnDrop::dir(staging.clone());
        let err = commit_extracted_dictionary(&dest, &staging, "en", &mut guard)
            .expect_err("incomplete extract must not publish");
        drop(guard);

        assert!(matches!(err, MonolingualError::Extraction(_)));
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"old");
        assert_eq!(fs::read(dest.join("dict.dict")).unwrap(), b"old");
        assert!(!replaced_dir(&root, "en").exists());
        assert!(!staging.exists());
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_publish_replaces_existing_dest_and_keeps_aside() {
        let (_service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let staging = staging_dir(&root, "en");
        let aside = replaced_dir(&root, "en");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"old").unwrap();
        fs::write(dest.join("dict.dict"), b"old").unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("dict.index"), b"new").unwrap();
        fs::write(staging.join("dict.dict"), b"new").unwrap();

        let mut guard = crate::fs::RemovePathOnDrop::dir(staging.clone());
        let previous = publish_extracted_dictionary(&dest, &staging, "en", &mut guard).unwrap();

        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"new");
        assert_eq!(previous.as_ref(), Some(&aside));
        assert!(aside.exists());
        assert_eq!(fs::read(aside.join("dict.index")).unwrap(), b"old");
        assert!(!staging.exists());

        discard_replaced_aside(previous.as_ref());
        assert!(!aside.exists());
    }

    #[test]
    fn test_restore_previous_dest_after_failed_registry_write() {
        let (_service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let staging = staging_dir(&root, "en");
        let aside = replaced_dir(&root, "en");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"old").unwrap();
        fs::write(dest.join("dict.dict"), b"old").unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("dict.index"), b"new").unwrap();
        fs::write(staging.join("dict.dict"), b"new").unwrap();

        let mut guard = crate::fs::RemovePathOnDrop::dir(staging.clone());
        let previous = publish_extracted_dictionary(&dest, &staging, "en", &mut guard).unwrap();
        restore_previous_dictionary_after_registry_failure(&dest, previous.as_ref()).unwrap();

        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"old");
        assert_eq!(fs::read(dest.join("dict.dict")).unwrap(), b"old");
        assert!(!aside.exists());
        assert!(!staging.exists());
    }

    #[test]
    fn test_reconcile_prefers_complete_staging_over_replaced_aside() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let staging = staging_dir(&root, "en");
        let aside = replaced_dir(&root, "en");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("dict.index"), b"new").unwrap();
        fs::write(staging.join("dict.dict"), b"new").unwrap();
        fs::create_dir_all(&aside).unwrap();
        fs::write(aside.join("dict.index"), b"old").unwrap();
        fs::write(aside.join("dict.dict"), b"old").unwrap();

        service.reconcile().unwrap();

        let dest = root.join("en");
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"new");
        assert!(!staging.exists());
        assert!(!aside.exists());
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_discards_replaced_aside_when_not_registered() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let aside = replaced_dir(&root, "en");
        fs::create_dir_all(&aside).unwrap();
        fs::File::create(aside.join("dict.index")).unwrap();
        fs::File::create(aside.join("dict.dict")).unwrap();

        service.reconcile().unwrap();

        assert!(!aside.exists());
        assert!(!dest.exists());
        assert!(service.get_installed_dictionaries().unwrap().is_empty());
    }

    #[test]
    fn test_reconcile_restores_replaced_aside_when_registered_and_dest_missing() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let aside = replaced_dir(&root, "en");
        fs::create_dir_all(&aside).unwrap();
        fs::File::create(aside.join("dict.index")).unwrap();
        fs::File::create(aside.join("dict.dict")).unwrap();
        service
            .db
            .record_install("en", UnixTimestamp::now())
            .unwrap();

        service.reconcile().unwrap();

        assert!(!aside.exists());
        assert!(has_dict_pair(&root.join("en")));
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_restores_replaced_aside_over_incomplete_dest() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let aside = replaced_dir(&root, "en");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"incomplete").unwrap();
        fs::create_dir_all(&aside).unwrap();
        fs::write(aside.join("dict.index"), b"complete").unwrap();
        fs::write(aside.join("dict.dict"), b"complete").unwrap();
        service
            .db
            .record_install("en", UnixTimestamp::now())
            .unwrap();

        service.reconcile().unwrap();

        assert!(!aside.exists());
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"complete");
        assert!(has_dict_pair(&dest));
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_keeps_complete_dest_when_aside_leftover_and_update_pending() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let aside = replaced_dir(&root, "en");
        let old_version: UnixTimestamp = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap().into();
        service
            .db
            .upsert_entry("en", &make_entry(2026, 4, 1))
            .unwrap();
        service.db.record_install("en", old_version).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"new").unwrap();
        fs::write(dest.join("dict.dict"), b"new").unwrap();
        fs::create_dir_all(&aside).unwrap();
        fs::write(aside.join("dict.index"), b"old").unwrap();
        fs::write(aside.join("dict.dict"), b"old").unwrap();

        assert!(service.is_update_available("en"));
        service.reconcile().unwrap();

        assert!(!aside.exists());
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"new");
        assert!(
            service.is_update_available("en"),
            "complete dest wins; leftover aside must not hide an update"
        );
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_promotes_staging_without_restamping_existing_install() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let staging = staging_dir(&root, "en");
        let old_version: UnixTimestamp = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap().into();
        service
            .db
            .upsert_entry("en", &make_entry(2026, 4, 1))
            .unwrap();
        service.db.record_install("en", old_version).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"incomplete").unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("dict.index"), b"new").unwrap();
        fs::write(staging.join("dict.dict"), b"new").unwrap();

        assert!(service.is_update_available("en"));
        service.reconcile().unwrap();

        assert!(!staging.exists());
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"new");
        assert!(
            service.is_update_available("en"),
            "promoting staging must keep the existing installed_version"
        );
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_reconcile_discards_aside_when_install_already_recorded() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let aside = replaced_dir(&root, "en");
        let version: UnixTimestamp = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap().into();
        service
            .db
            .upsert_entry("en", &make_entry(2026, 4, 1))
            .unwrap();
        service.db.record_install("en", version).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"new").unwrap();
        fs::write(dest.join("dict.dict"), b"new").unwrap();
        fs::create_dir_all(&aside).unwrap();
        fs::write(aside.join("dict.index"), b"old").unwrap();
        fs::write(aside.join("dict.dict"), b"old").unwrap();

        assert!(!service.is_update_available("en"));
        service.reconcile().unwrap();

        assert!(!aside.exists());
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"new");
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_staging_and_replaced_lang_reject_empty_names() {
        assert_eq!(staging_lang(".en.partial"), Some("en"));
        assert_eq!(replaced_lang(".en.replaced"), Some("en"));
        assert_eq!(staging_lang("..partial"), None);
        assert_eq!(replaced_lang("..replaced"), None);
        assert_eq!(staging_lang(".partial"), None);
        assert_eq!(replaced_lang(".replaced"), None);
    }

    #[test]
    fn test_reconcile_ignores_empty_lang_staging_dir() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let empty_lang_staging = root.join("..partial");
        let keep = root.join("en");
        fs::create_dir_all(&empty_lang_staging).unwrap();
        fs::write(empty_lang_staging.join("dict.index"), b"x").unwrap();
        fs::write(empty_lang_staging.join("dict.dict"), b"x").unwrap();
        fs::create_dir_all(&keep).unwrap();
        fs::write(keep.join("dict.index"), b"en").unwrap();
        fs::write(keep.join("dict.dict"), b"en").unwrap();

        service.reconcile().unwrap();

        assert!(
            empty_lang_staging.exists(),
            "malformed empty-lang staging must not be treated as a language"
        );
        assert!(
            has_dict_pair(&keep),
            "existing language dirs must survive empty-lang staging"
        );
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
        assert!(root.exists());
    }

    #[test]
    fn test_reconcile_ignores_dot_prefixed_dest_dir() {
        let (service, dir, _db) = create_test_service();
        let hidden = dir.path().join(READER_DICT_SUBDIR).join(".hidden");
        fs::create_dir_all(&hidden).unwrap();
        fs::write(hidden.join("dict.index"), b"x").unwrap();
        fs::write(hidden.join("dict.dict"), b"x").unwrap();

        service.reconcile().unwrap();

        assert!(hidden.exists());
        assert!(service.get_installed_dictionaries().unwrap().is_empty());
    }

    #[test]
    fn test_reconcile_restored_aside_keeps_existing_install_version() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let aside = replaced_dir(&root, "en");
        let old_version: UnixTimestamp = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap().into();
        service
            .db
            .upsert_entry("en", &make_entry(2026, 4, 1))
            .unwrap();
        service.db.record_install("en", old_version).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("dict.index"), b"incomplete").unwrap();
        fs::create_dir_all(&aside).unwrap();
        fs::write(aside.join("dict.index"), b"old").unwrap();
        fs::write(aside.join("dict.dict"), b"old").unwrap();

        assert!(service.is_update_available("en"));
        service.reconcile().unwrap();

        assert!(!aside.exists());
        assert_eq!(fs::read(dest.join("dict.index")).unwrap(), b"old");
        assert!(
            service.is_update_available("en"),
            "restoring aside must keep the older installed_version"
        );
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["en".to_string()]
        );
    }

    #[test]
    fn test_install_record_and_restore_error_includes_both_failures() {
        let err = MonolingualError::InstallRecordAndRestore {
            registry: "db locked".to_string(),
            restore: "permission denied".to_string(),
        };
        let message = err.to_string();
        assert!(
            message.contains("db locked") && message.contains("permission denied"),
            "combined error must keep both failures, got {message}"
        );
    }

    #[test]
    fn test_restore_previous_fails_when_dest_cannot_be_removed() {
        let (_service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let aside = replaced_dir(&root, "en");
        fs::create_dir_all(&root).unwrap();
        fs::write(&dest, b"not-a-directory").unwrap();
        fs::create_dir_all(&aside).unwrap();
        fs::write(aside.join("dict.index"), b"old").unwrap();
        fs::write(aside.join("dict.dict"), b"old").unwrap();

        restore_previous_dictionary_after_registry_failure(&dest, Some(&aside))
            .expect_err("file dest must block restore");
        assert!(dest.is_file());
        assert!(aside.exists());
    }

    #[test]
    fn test_reconcile_continues_after_staging_failure() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        fs::create_dir_all(&root).unwrap();

        let blocked_dest = root.join("en");
        fs::write(&blocked_dest, b"not-a-directory").unwrap();
        let blocked_staging = staging_dir(&root, "en");
        fs::create_dir_all(&blocked_staging).unwrap();
        fs::write(blocked_staging.join("dict.index"), b"new").unwrap();
        fs::write(blocked_staging.join("dict.dict"), b"new").unwrap();

        let ok_staging = staging_dir(&root, "fr");
        fs::create_dir_all(&ok_staging).unwrap();
        fs::write(ok_staging.join("dict.index"), b"fr").unwrap();
        fs::write(ok_staging.join("dict.dict"), b"fr").unwrap();

        let err = service
            .reconcile()
            .expect_err("blocked staging promotion must surface after other work");

        assert!(
            matches!(err, MonolingualError::ReconcileIncomplete { count: 1, .. }),
            "expected one aggregated failure, got {err:?}"
        );
        assert!(
            !ok_staging.exists(),
            "successful staging must still be promoted"
        );
        assert!(has_dict_pair(&root.join("fr")));
        assert_eq!(
            service.get_installed_dictionaries().unwrap(),
            vec!["fr".to_string()]
        );
        assert!(
            blocked_staging.exists(),
            "failed staging should remain for a later retry"
        );
    }

    #[test]
    fn test_remove_installed_clears_dest_and_aside_so_reconcile_cannot_restore() {
        let (service, dir, _db) = create_test_service();
        let root = dir.path().join(READER_DICT_SUBDIR);
        let dest = root.join("en");
        let staging = staging_dir(&root, "en");
        let aside = replaced_dir(&root, "en");
        fs::create_dir_all(&dest).unwrap();
        fs::File::create(dest.join("dict.index")).unwrap();
        fs::File::create(dest.join("dict.dict")).unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::File::create(staging.join("dict.index")).unwrap();
        fs::File::create(staging.join("dict.dict")).unwrap();
        fs::create_dir_all(&aside).unwrap();
        fs::File::create(aside.join("dict.index")).unwrap();
        fs::File::create(aside.join("dict.dict")).unwrap();
        service
            .db
            .record_install("en", UnixTimestamp::now())
            .unwrap();

        service.remove_installed("en");
        service.reconcile().unwrap();

        assert!(!aside.exists());
        assert!(!staging.exists());
        assert!(!dest.exists());
        assert!(service.get_installed_dictionaries().unwrap().is_empty());
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
