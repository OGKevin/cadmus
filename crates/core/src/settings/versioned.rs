//! Settings versioning and migration support.
//!
//! This module provides infrastructure for maintaining backward and forward
//! compatibility across application versions. Settings are stored in a
//! versioned directory structure with a manifest file tracking all versions.
//!
//! # Directory Structure
//!
//! ```text
//! Settings/
//! ├── .cadmus-index.toml        # Manifest file with version metadata
//! ├── Settings-v0.1.2.toml      # Version-specific settings files
//! ├── Settings-v0.1.3-5-gabc123.toml
//! └── Settings-v0.2.0.toml
//! ```
//!
//! # Migration Strategy
//!
//! When the application loads:
//! 1. Check for legacy `Settings.toml` in the root directory
//! 2. If it exists, migrate it to the versioned system and delete the old file
//! 3. Read the manifest to find the most recent version
//! 4. Load that version's settings file
//! 5. If the current version differs, copy to new version file
//!
//! When the application saves:
//! 1. Write to the current version file
//! 2. Update manifest metadata
//! 3. Remove old files exceeding retention limit
use crate::settings::Settings;
use anyhow::{Context, Error};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const SETTINGS_DIR: &str = "Settings";
const MANIFEST_FILE: &str = ".cadmus-index.toml";
const LEGACY_SETTINGS_FILE: &str = "Settings.toml";

/// Metadata for a settings file version in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsEntry {
    /// The version string (e.g., "v0.1.2" or "v0.1.3-5-gabc123").
    pub version: String,
    /// UUID v7 from the build that created this entry (timestamp-sortable).
    pub uuid: String,
    /// Path to the settings file (relative to Settings directory).
    pub file: String,
    /// When this settings file was last saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}

/// Manifest file that tracks all settings versions.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SettingsManifest {
    /// All known settings versions, in order.
    #[serde(default)]
    pub entries: Vec<SettingsEntry>,
}

/// Manages versioned settings files and migrations.
#[derive(Clone)]
pub struct SettingsManager {
    settings_dir: PathBuf,
    manifest_path: PathBuf,
    current_version: String,
    build_uuid: String,
    root_dir: PathBuf,
}

impl SettingsManager {
    /// Creates a new settings manager.
    ///
    /// # Arguments
    ///
    /// * `current_version` - The current application version (e.g., from `GIT_VERSION`)
    ///
    /// The build UUID is automatically obtained from the compile-time `BUILD_UUID`
    /// environment variable set by the core crate's build script.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cadmus_core::settings::versioned::SettingsManager;
    ///
    /// let manager = SettingsManager::new(env!("GIT_VERSION").to_string());
    /// ```
    pub fn new(current_version: String) -> Self {
        let root_dir = PathBuf::from(".");
        let settings_dir = root_dir.join(SETTINGS_DIR);
        let manifest_path = settings_dir.join(MANIFEST_FILE);

        SettingsManager {
            settings_dir,
            manifest_path,
            current_version,
            build_uuid: env!("BUILD_UUID").to_string(),
            root_dir,
        }
    }

    /// Loads settings from the versioned storage, migrating if necessary.
    ///
    /// This function is designed to be maximally resilient:
    /// 1. Creates Settings directory if it doesn't exist
    /// 2. Attempts to migrate legacy Settings.toml if it exists (non-fatal if it fails)
    /// 3. Reads the manifest to find the appropriate settings file
    /// 4. Loads and deserializes the settings
    ///
    /// The manifest is searched for an entry matching the current version.
    /// If no exact match exists, the entry with the most recent UUID is used.
    /// If the manifest is empty, default settings are returned.
    ///
    /// # Returns
    ///
    /// Returns `Settings` in all cases:
    /// - Loaded from versioned file if available
    /// - Loaded from most recent version if exact match not found
    /// - Default settings if no versions exist or all file reads fail
    ///
    /// Never fails - returns defaults as ultimate fallback.
    ///
    /// # Diagnostic Output
    ///
    /// This function uses `println!` and `eprintln!` for diagnostic messages
    /// instead of `tracing::*` macros because logging/tracing is not yet
    /// configured at this point in the app startup sequence. Tracing is
    /// configured *after* settings are loaded, so using tracing macros here
    /// would result in messages being silently dropped or not properly routed.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level = tracing::Level::TRACE)))]
    pub fn load(&self) -> Settings {
        if let Err(e) = fs::create_dir_all(&self.settings_dir) {
            eprintln!("failed to create settings directory: {}; using defaults", e);
            return Settings::default();
        }

        self.migrate_legacy_settings();

        let manifest = match self.read_manifest() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("failed to read manifest: {}; using defaults", e);
                return Settings::default();
            }
        };

        let matched_entry = manifest
            .entries
            .iter()
            .find(|e| e.version == self.current_version)
            .cloned()
            .or_else(|| {
                let mut entries: Vec<_> = manifest.entries.clone();
                entries.sort_by(|a, b| b.uuid.cmp(&a.uuid));
                entries.first().cloned()
            });

        match matched_entry {
            Some(entry) => {
                println!(
                    "Loading settings from version {} (file: {})",
                    entry.version, entry.file
                );
                let file_path = self.settings_dir.join(&entry.file);
                match crate::helpers::load_toml::<Settings, _>(&file_path) {
                    Ok(settings) => settings,
                    Err(e) => {
                        eprintln!(
                            "failed to load settings file {}: {}; using defaults",
                            file_path.display(),
                            e
                        );
                        Settings::default()
                    }
                }
            }
            None => {
                println!(
                    "No existing settings found for version {}, using defaults",
                    self.current_version
                );
                Settings::default()
            }
        }
    }

    /// Saves settings to a versioned file and updates the manifest.
    ///
    /// This function:
    /// 1. Creates a new Settings-<version>.toml file
    /// 2. Updates the manifest with new entry
    /// 3. Removes old files exceeding retention limit
    ///
    /// # Arguments
    ///
    /// * `settings` - The settings to save
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Settings file cannot be written
    /// - Manifest cannot be updated
    /// - Old files cannot be removed
    #[cfg_attr(
        feature = "otel", tracing::instrument(
            skip(self, settings),
            fields(
                version = %self.current_version,
                settings_dir = %self.settings_dir.display(),
                build_uuid = %self.build_uuid
            ),
            ret(level = tracing::Level::TRACE)
        )
    )]
    pub fn save(&self, settings: &Settings) -> Result<(), Error> {
        tracing::debug!(settings_dir = %self.settings_dir.display(), "creating settings directory");
        fs::create_dir_all(&self.settings_dir).context("failed to create settings directory")?;

        let filename = format!("Settings-{}.toml", self.current_version);
        let file_path = self.settings_dir.join(&filename);

        tracing::debug!(file_path = %file_path.display(), "saving settings to file");
        crate::helpers::save_toml(settings, &file_path).context("failed to save settings file")?;

        let file_size = file_path.metadata().ok().map(|m| m.len());

        tracing::info!(
            version = %self.current_version,
            file = %filename,
            file_path = %file_path.display(),
            file_size = ?file_size,
            "Saved versioned settings"
        );

        self.update_manifest_and_cleanup(&filename, settings)?;

        Ok(())
    }

    /// Migrates legacy Settings.toml from the root directory to the new versioned format.
    ///
    /// This method is automatically called during `load()` to handle upgrades from the
    /// old settings system. If a legacy `Settings.toml` file exists in the application's
    /// root directory, it is:
    ///
    /// 1. Loaded with all existing settings preserved
    /// 2. Saved to the new versioned location: `Settings/Settings-v<version>.toml`
    /// 3. Registered in the manifest as a historical entry
    /// 4. Deleted to prevent accidental duplication
    ///
    /// # Behavior
    ///
    /// This method is fully non-fatal:
    /// - If the legacy file doesn't exist, returns silently (success)
    /// - If the legacy file can't be read, logs a warning and returns (failure is acceptable)
    /// - If any write operation fails (save, manifest update, deletion), logs a warning
    ///   but continues - the important part is reading the legacy settings, not cleanup
    ///
    /// Never propagates errors because migration is opportunistic, not required for
    /// the app to function.
    ///
    /// # Diagnostic Output
    ///
    /// This function uses `println!` and `eprintln!` for diagnostic messages
    /// instead of `tracing::*` macros because logging/tracing is not yet
    /// configured during the settings loading phase (called from `load()`).
    /// Tracing is initialized *after* settings are fully loaded, so any
    /// tracing calls here would be silently discarded.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level = tracing::Level::TRACE)))]
    fn migrate_legacy_settings(&self) {
        let legacy_path = self.root_dir.join(LEGACY_SETTINGS_FILE);

        if !legacy_path.exists() {
            return;
        }

        println!(
            "Migrating legacy settings from {} to versioned format",
            legacy_path.display()
        );

        let settings = match crate::helpers::load_toml::<Settings, _>(&legacy_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "failed to load legacy settings file {}: {}; skipping migration",
                    legacy_path.display(),
                    e
                );
                return;
            }
        };

        let filename = format!("Settings-{}.toml", self.current_version);
        let file_path = self.settings_dir.join(&filename);

        if let Err(e) = crate::helpers::save_toml(&settings, &file_path) {
            eprintln!(
                "Failed to save migrated settings file {}: {}; continuing with legacy",
                file_path.display(),
                e
            );
            return;
        }

        let mut manifest = self.read_manifest().unwrap_or_default();

        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

        manifest
            .entries
            .retain(|e| e.version != self.current_version);

        let new_entry = SettingsEntry {
            version: self.current_version.clone(),
            uuid: self.build_uuid.clone(),
            file: filename,
            saved_at: Some(now),
        };

        manifest.entries.push(new_entry);

        if let Err(e) = self.write_manifest(&manifest) {
            eprintln!(
                "Failed to update manifest after migration: {}; continuing",
                e
            );
        }

        if let Err(e) = fs::remove_file(&legacy_path) {
            eprintln!(
                "Failed to delete legacy {} after migration: {}; continuing",
                legacy_path.display(),
                e
            );
        }

        println!(
            "Successfully migrated legacy settings to version {} (file: {})",
            self.current_version,
            file_path.display()
        );
    }

    /// Reads the settings manifest from disk.
    ///
    /// The manifest file (`.cadmus-index.toml`) tracks all known settings versions
    /// and their metadata. This method loads the current manifest or returns a default
    /// empty manifest if the file doesn't exist.
    ///
    /// # Returns
    ///
    /// `Ok(SettingsManifest)` containing:
    /// - All known settings file entries in order
    /// - Version information and timestamps for each entry
    ///
    /// `Err` if the manifest file exists but cannot be read or parsed.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level = tracing::Level::TRACE)))]
    fn read_manifest(&self) -> Result<SettingsManifest, Error> {
        if self.manifest_path.exists() {
            crate::helpers::load_toml::<SettingsManifest, _>(&self.manifest_path)
                .context("failed to read settings manifest")
        } else {
            Ok(SettingsManifest::default())
        }
    }

    /// Writes the settings manifest to disk.
    ///
    /// Persists the manifest file (`.cadmus-index.toml`) with all settings version
    /// entries and their metadata. This is called after any changes to the manifest
    /// (migration, version updates, cleanup).
    ///
    /// # Arguments
    ///
    /// * `manifest` - The manifest to write to disk
    ///
    /// # Returns
    ///
    /// `Ok(())` if the manifest was successfully written.
    ///
    /// `Err` if the manifest file cannot be written or serialized.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, manifest), ret(level = tracing::Level::TRACE)))]
    fn write_manifest(&self, manifest: &SettingsManifest) -> Result<(), Error> {
        crate::helpers::save_toml(manifest, &self.manifest_path)
            .context("failed to write settings manifest")
    }

    /// Updates the manifest with a new settings version entry and cleans up old files.
    ///
    /// This is called during `save()` when settings are persisted to a new versioned file.
    /// Combines manifest update and cleanup into a single I/O pass to minimize filesystem
    /// operations on embedded devices with slow storage.
    ///
    /// The process:
    /// 1. Reads the current manifest (single read)
    /// 2. Creates a new entry for the current version with timestamp
    /// 3. Removes any existing entry for the same version (deduplication)
    /// 4. Appends the new entry
    /// 5. Removes old files exceeding retention limit
    /// 6. Writes the updated manifest once (single write)
    ///
    /// # Arguments
    ///
    /// * `filename` - The filename of the new settings file (relative to Settings directory)
    /// * `settings` - The settings containing the `settings_retention` configuration
    ///
    /// # Returns
    ///
    /// `Ok(())` if the manifest was successfully updated and written.
    ///
    /// `Err` if reading, updating, or writing the manifest fails.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, settings), fields(filename = filename), ret(level = tracing::Level::TRACE)))]
    fn update_manifest_and_cleanup(
        &self,
        filename: &str,
        settings: &Settings,
    ) -> Result<(), Error> {
        let mut manifest = self.read_manifest()?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

        manifest
            .entries
            .retain(|e| e.version != self.current_version);

        let new_entry = SettingsEntry {
            version: self.current_version.clone(),
            uuid: self.build_uuid.clone(),
            file: filename.to_string(),
            saved_at: Some(now),
        };

        manifest.entries.push(new_entry);

        let retention = settings.settings_retention;

        if retention > 0 && manifest.entries.len() > retention {
            manifest.entries.sort_by(|a, b| a.uuid.cmp(&b.uuid));

            let entries_to_remove = manifest.entries.len() - retention;
            let removed_entries: Vec<_> = manifest.entries.drain(..entries_to_remove).collect();

            for entry in removed_entries {
                let file_path = self.settings_dir.join(&entry.file);

                if file_path.exists() {
                    fs::remove_file(&file_path).context(format!(
                        "failed to remove old settings file: {}",
                        entry.file
                    ))?;
                    tracing::debug!(
                        version = %entry.version,
                        file = %entry.file,
                        "Removed old settings file"
                    );
                }
            }
        }

        self.write_manifest(&manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    impl SettingsManager {
        fn clone_with_version(&self, version: String) -> Self {
            SettingsManager {
                settings_dir: self.settings_dir.clone(),
                manifest_path: self.manifest_path.clone(),
                current_version: version,
                build_uuid: self.build_uuid.clone(),
                root_dir: self.root_dir.clone(),
            }
        }
    }

    fn create_test_manager(temp_dir: &TempDir) -> SettingsManager {
        let root_dir = temp_dir.path().to_path_buf();
        let settings_dir = root_dir.join(SETTINGS_DIR);
        let manifest_path = settings_dir.join(MANIFEST_FILE);

        SettingsManager {
            settings_dir,
            manifest_path,
            current_version: "v0.1.0".to_string(),
            build_uuid: "018e1234567890abcdef".to_string(),
            root_dir,
        }
    }

    fn create_test_manager_with_root(temp_dir: &TempDir) -> (SettingsManager, PathBuf) {
        let manager = create_test_manager(temp_dir);
        (manager.clone(), manager.root_dir.clone())
    }

    #[test]
    fn test_creates_settings_directory() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir);

        let settings = manager.load();
        assert!(manager.settings_dir.exists());
        assert_eq!(settings.selected_library, 0);
    }

    #[test]
    fn test_manifest_is_created_on_save() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir);

        let settings = Settings::default();
        manager.save(&settings).unwrap();

        assert!(manager.manifest_path.exists());

        let manifest = manager.read_manifest().unwrap();
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].version, "v0.1.0");
    }

    #[test]
    fn test_settings_file_is_created() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir);

        let settings = Settings::default();
        manager.save(&settings).unwrap();

        let expected_file = manager.settings_dir.join("Settings-v0.1.0.toml");
        assert!(expected_file.exists());
    }

    #[test]
    fn test_load_existing_file_same_version() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir);

        let settings = manager.load();
        assert_eq!(settings.selected_library, 0, "Should load defaults");

        let manifest = manager.read_manifest().unwrap();
        assert!(
            manifest.entries.is_empty(),
            "Manifest should be empty with no legacy file"
        );
    }

    #[test]
    fn test_legacy_migration_preserves_manifest_history() {
        let temp_dir = TempDir::new().unwrap();
        let (mut manager, root) = create_test_manager_with_root(&temp_dir);

        let legacy_settings = Settings {
            selected_library: 1,
            ..Settings::default()
        };
        let legacy_path = root.join(LEGACY_SETTINGS_FILE);
        crate::helpers::save_toml(&legacy_settings, &legacy_path).unwrap();

        let settings = manager.load();

        manager.current_version = "v0.1.1".to_string();
        manager.save(&settings).unwrap();

        let loaded = manager.load();
        assert_eq!(
            loaded.selected_library, 1,
            "Second load should still work correctly"
        );
    }

    #[test]
    fn test_same_version_multiple_saves_updates_entry() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir);

        let mut settings = Settings {
            selected_library: 1,
            ..Settings::default()
        };

        manager.save(&settings).unwrap();

        let file_path = manager
            .settings_dir
            .join(format!("Settings-{}.toml", manager.current_version));

        assert!(
            file_path.exists(),
            "Settings file should exist after first save"
        );

        let manifest = manager.read_manifest().unwrap();
        assert_eq!(
            manifest.entries.len(),
            1,
            "Manifest should have 1 entry after first save"
        );
        assert_eq!(manifest.entries[0].version, "v0.1.0");

        settings.selected_library = 2;
        manager.save(&settings).unwrap();

        assert!(
            file_path.exists(),
            "Settings file should still exist after second save with same version"
        );

        let manifest = manager.read_manifest().unwrap();
        assert_eq!(
            manifest.entries.len(),
            1,
            "Manifest should still have 1 entry (same version replaces previous)"
        );
        assert_eq!(manifest.entries[0].version, "v0.1.0");

        // Verify the settings were updated by loading
        let loaded = manager.load();
        assert_eq!(
            loaded.selected_library, 2,
            "Settings should reflect the second save"
        );
    }

    #[test]
    fn test_load_falls_back_to_most_recent_by_uuid() {
        let temp_dir = TempDir::new().unwrap();
        let root_dir = temp_dir.path().to_path_buf();
        let settings_dir = root_dir.join(SETTINGS_DIR);
        let manifest_path = settings_dir.join(MANIFEST_FILE);

        // Create manager for v0.1.0 with an older UUID (smaller timestamp)
        let manager_v1 = SettingsManager {
            settings_dir: settings_dir.clone(),
            manifest_path: manifest_path.clone(),
            current_version: "v0.1.0".to_string(),
            build_uuid: "018e0000000000000000".to_string(), // Older UUID
            root_dir: root_dir.clone(),
        };

        let settings_v1 = Settings {
            selected_library: 1,
            ..Settings::default()
        };
        manager_v1.save(&settings_v1).unwrap();

        // Create manager for v0.2.0 with a newer UUID (larger timestamp)
        let manager_v2 = SettingsManager {
            settings_dir: settings_dir.clone(),
            manifest_path: manifest_path.clone(),
            current_version: "v0.2.0".to_string(),
            build_uuid: "018effffffffffffffff".to_string(), // Newer UUID
            root_dir: root_dir.clone(),
        };

        let settings_v2 = Settings {
            selected_library: 2,
            ..Settings::default()
        };
        manager_v2.save(&settings_v2).unwrap();

        // Create manager for v0.3.0 with a different UUID (no settings saved)
        // This should fall back to the most recent settings by UUID (v0.2.0)
        let manager_v3 = SettingsManager {
            settings_dir,
            manifest_path,
            current_version: "v0.3.0".to_string(),
            build_uuid: "018eaaaaaaaaaaaaaaaa".to_string(), // Different UUID
            root_dir,
        };

        let loaded = manager_v3.load();

        assert_eq!(
            loaded.selected_library, 2,
            "v0.3.0 should load settings from v0.2.0 (most recent by UUID)"
        );
    }

    #[test]
    fn test_load_uses_exact_version_match_when_available() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir);

        // Create settings for v0.1.0 with selected_library = 1
        let settings_v1 = Settings {
            selected_library: 1,
            ..Settings::default()
        };
        manager.save(&settings_v1).unwrap();

        // Create a new manager simulating v0.2.0 with a newer UUID and different settings
        let manager_v2 = manager.clone_with_version("v0.2.0".to_string());
        let settings_v2 = Settings {
            selected_library: 2,
            ..Settings::default()
        };
        manager_v2.save(&settings_v2).unwrap();

        // Load as v0.1.0 - should find exact match and use v0.1.0 settings
        let manager_v1_reload = manager.clone_with_version("v0.1.0".to_string());
        let loaded = manager_v1_reload.load();

        assert_eq!(
            loaded.selected_library, 1,
            "v0.1.0 should load its own settings (exact match), not v0.2.0"
        );
    }

    #[test]
    fn test_migration_succeeds_even_if_legacy_deletion_fails() {
        let temp_dir = TempDir::new().unwrap();
        let (manager, root) = create_test_manager_with_root(&temp_dir);

        let legacy_settings = Settings {
            selected_library: 5,
            ..Settings::default()
        };
        let legacy_path = root.join(LEGACY_SETTINGS_FILE);
        crate::helpers::save_toml(&legacy_settings, &legacy_path).unwrap();

        assert!(legacy_path.exists(), "Legacy settings file should exist");

        let loaded = manager.load();

        assert_eq!(
            loaded.selected_library, 5,
            "Migration should succeed and load settings even if deletion fails"
        );

        let versioned_file = manager.settings_dir.join("Settings-v0.1.0.toml");
        assert!(
            versioned_file.exists(),
            "Versioned settings file should be created"
        );

        let manifest = manager.read_manifest().unwrap();
        assert_eq!(
            manifest.entries.len(),
            1,
            "Manifest should have migrated entry"
        );
        assert_eq!(manifest.entries[0].version, "v0.1.0");
    }

    #[test]
    fn test_load_returns_defaults_on_all_failures() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir);

        let loaded = manager.load();

        assert_eq!(loaded.selected_library, 0, "Should return defaults");
        assert_eq!(
            loaded.keyboard_layout, "English",
            "Should return default keyboard layout"
        );
    }

    #[test]
    fn test_migration_deduplicated_on_retry() {
        let temp_dir = TempDir::new().unwrap();
        let (manager, root) = create_test_manager_with_root(&temp_dir);

        let legacy_settings = Settings {
            selected_library: 3,
            ..Settings::default()
        };
        let legacy_path = root.join(LEGACY_SETTINGS_FILE);
        crate::helpers::save_toml(&legacy_settings, &legacy_path).unwrap();

        let loaded1 = manager.load();
        assert_eq!(
            loaded1.selected_library, 3,
            "First load should get legacy settings"
        );

        let manifest = manager.read_manifest().unwrap();
        let first_entry_count = manifest.entries.len();

        let loaded2 = manager.load();
        assert_eq!(
            loaded2.selected_library, 3,
            "Second load should still get correct settings"
        );

        let manifest = manager.read_manifest().unwrap();
        assert_eq!(
            manifest.entries.len(),
            first_entry_count,
            "Manifest should not have duplicates on retry"
        );
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir);

        let settings = Settings {
            selected_library: 5,
            inverted: true,
            ..Settings::default()
        };

        manager.save(&settings).unwrap();

        let loaded = manager.load();
        assert_eq!(
            loaded.selected_library, 5,
            "Should save and load selected_library"
        );
        assert!(loaded.inverted, "Should save and load inverted");
    }

    #[test]
    fn test_retention_cleanup_removes_oldest_by_uuid() {
        let temp_dir = TempDir::new().unwrap();
        let (manager, root) = create_test_manager_with_root(&temp_dir);

        let settings = Settings {
            settings_retention: 2,
            ..Settings::default()
        };

        let managers = [
            ("v0.1.0", "018e0000000000000000"),
            ("v0.1.1", "018e5555555555555555"),
            ("v0.1.2", "018effffffffffffffff"),
        ];

        for (version, uuid) in managers {
            let mgr = SettingsManager {
                settings_dir: manager.settings_dir.clone(),
                manifest_path: manager.manifest_path.clone(),
                current_version: version.to_string(),
                build_uuid: uuid.to_string(),
                root_dir: root.clone(),
            };

            mgr.save(&settings).unwrap();
        }

        let manifest_path = manager.manifest_path.clone();
        let manifest_content = std::fs::read_to_string(&manifest_path).unwrap();

        assert!(
            manifest_content.contains("v0.1.1"),
            "Oldest entry (v0.1.0) should be removed, v0.1.1 should remain"
        );
        assert!(
            manifest_content.contains("v0.1.2"),
            "Newest entry (v0.1.2) should be kept"
        );
        assert!(
            !manifest_content.contains("018e0000000000000000"),
            "Settings file for oldest UUID should be deleted"
        );
    }
}
