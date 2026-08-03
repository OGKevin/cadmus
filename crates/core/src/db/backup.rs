use crate::db::version::{MigrationHash, current_migration_hash};
use crate::version::GitVersion;
use anyhow::{Context, Error};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Subdirectory under the database directory where backups are stored.
const BACKUP_DIR: &str = "backups";
/// Filename of the TOML manifest that tracks all known backups.
const MANIFEST_FILE: &str = ".cadmus-db-index.toml";

/// File suffixes for SQLite companion files (WAL and shared-memory).
const SQLITE_COMPANION_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// Manifest that tracks all database backups.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BackupManifest {
    /// All known database backups, in creation order.
    #[serde(default)]
    pub entries: Vec<BackupEntry>,
}

/// Metadata for a single database backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    /// The Cadmus version that created this backup.
    pub version: GitVersion,
    /// The backup filename (relative to the backup directory).
    pub file: String,
    /// The UTC datetime when the backup was created.
    pub created_at: DateTime<Utc>,
    /// The schema migration hash embedded in the build that created this backup.
    pub migration_hash: MigrationHash,
}

/// Errors that can occur when restoring a database backup.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    /// No backup exists in the manifest for the target version.
    #[error("no backup found in manifest for version {version}")]
    NoBackupFound { version: GitVersion },
    /// The manifest references a backup file that no longer exists on disk.
    ///
    /// Under normal operation this variant is unreachable: [`DbBackupManager::find_best_backup`]
    /// filters out entries with missing files before a restore is attempted.
    /// Receiving this error indicates an invariant violation (e.g. the file was
    /// removed between the lookup and the restore).
    #[error("backup file '{file}' referenced by manifest does not exist on disk")]
    BackupFileMissing { file: String },
    /// An I/O or other low-level error occurred during the restore.
    #[error(transparent)]
    Io(#[from] Error),
}

/// Manages versioned SQLite database backups.
#[derive(Clone)]
pub struct DbBackupManager {
    db_dir: PathBuf,
    current_version: GitVersion,
}

impl DbBackupManager {
    /// Creates a backup manager for the database directory.
    ///
    /// `db_dir` is the directory containing `cadmus.sqlite`. Backups are stored
    /// in `db_dir/backups/`.
    pub fn new(db_dir: PathBuf, current_version: GitVersion) -> Self {
        Self {
            db_dir,
            current_version,
        }
    }

    /// Returns the path to the backup directory.
    fn backup_dir(&self) -> PathBuf {
        self.db_dir.join(BACKUP_DIR)
    }

    /// Returns the path to the backup manifest.
    fn manifest_path(&self) -> PathBuf {
        self.backup_dir().join(MANIFEST_FILE)
    }

    /// Creates a backup of the current database via WAL checkpoint and filesystem copy.
    ///
    /// Checkpoints the WAL into the main file first so the copy is a consistent
    /// snapshot, then copies `source_path` (and any remaining companion files)
    /// to `backups/cadmus-v<version>.sqlite`. The manifest is updated and old
    /// backups exceeding the retention limit are removed.
    ///
    /// `source_path` must be the active database file opened by `pool`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn create_backup(
        &self,
        pool: &SqlitePool,
        source_path: &Path,
        retention: usize,
    ) -> Result<PathBuf, Error> {
        if retention == 0 {
            return Err(anyhow::anyhow!(
                "cannot create backup: db_backup_retention is 0"
            ));
        }

        fs::create_dir_all(self.backup_dir())
            .await
            .context("failed to create backup directory")?;

        let filename = format!("cadmus-{}.sqlite", self.current_version);
        let backup_path = self.backup_dir().join(&filename);
        let tmp_path = self.backup_dir().join(format!("{filename}.tmp"));

        remove_sqlite_files(&tmp_path)
            .await
            .context("failed to clean up temporary backup files")?;

        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(pool)
            .await
            .context("failed to run PRAGMA wal_checkpoint before backup")?;

        copy_sqlite_files(source_path, &tmp_path)
            .await
            .context("failed to copy database files for backup")?;

        rename_sqlite_files(&tmp_path, &backup_path)
            .await
            .context("failed to promote completed backup")?;

        let created_at = Utc::now();
        let migration_hash = current_migration_hash();

        self.update_manifest_and_cleanup(&filename, created_at, migration_hash, retention)
            .await?;

        tracing::info!(
            version = %self.current_version,
            file = %filename,
            path = %backup_path.display(),
            "created database backup"
        );

        Ok(backup_path)
    }

    /// Restores the best available backup for the target version.
    ///
    /// The restore is performed in three steps to avoid losing the active
    /// database if an error occurs mid-way:
    ///
    /// 1. Copy the backup to a staging path (`cadmus-v<version>-restore-staged.sqlite`).
    /// 2. Rename the active database to `cadmus-v<newer_version>-demoted.sqlite`.
    /// 3. Rename the staged copy to the active path.
    ///
    /// If step 3 fails the demoted database is renamed back to `active_path` as
    /// a best-effort rollback. Demoted files are not tracked in the manifest and
    /// are never automatically deleted — they remain as a safety net for manual
    /// recovery.
    ///
    /// Returns the path of the restored backup file on success.
    ///
    /// # Errors
    ///
    /// - [`RestoreError::NoBackupFound`] — no manifest entry exists for the target version.
    /// - [`RestoreError::BackupFileMissing`] — the manifest references a file that is absent on disk.
    /// - [`RestoreError::Io`] — an I/O or filesystem error occurred during the restore.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn restore_best_backup(
        &self,
        active_path: &Path,
        newer_version: &GitVersion,
    ) -> Result<PathBuf, RestoreError> {
        let Some(entry) = self.find_best_backup(&self.current_version)? else {
            tracing::warn!(
                target_version = %self.current_version,
                "no database backup found for downgrade; continuing with current database"
            );
            return Err(RestoreError::NoBackupFound {
                version: self.current_version.clone(),
            });
        };

        let backup_path = self.backup_dir().join(&entry.file);
        if !backup_path.exists() {
            tracing::error!(
                file = %entry.file,
                path = %backup_path.display(),
                "backup file referenced by manifest does not exist (invariant violation)"
            );
            return Err(RestoreError::BackupFileMissing {
                file: entry.file.clone(),
            });
        }

        let demoted_filename = format!("cadmus-{}-demoted.sqlite", newer_version);
        let demoted_path = self.backup_dir().join(&demoted_filename);

        let staged_path = self.backup_dir().join(format!(
            "cadmus-{}-restore-staged.sqlite",
            self.current_version
        ));
        remove_sqlite_files(&staged_path)
            .await
            .context("failed to clean up staged restore files")?;

        copy_sqlite_files(&backup_path, &staged_path)
            .await
            .context("failed to stage backup files for restore")?;

        rename_sqlite_files(active_path, &demoted_path)
            .await
            .context("failed to demote active database")?;

        if let Err(e) = rename_sqlite_files(&staged_path, active_path).await {
            if let Err(rollback_err) = rename_sqlite_files(&demoted_path, active_path).await {
                tracing::error!(
                    error = %rollback_err,
                    "failed to roll back demoted database after restore promotion failure"
                );
            }
            return Err(RestoreError::Io(
                e.context("failed to promote staged restore"),
            ));
        }

        tracing::info!(
            was_version = %newer_version,
            restored_version = %entry.version,
            backup_file = %entry.file,
            "restored database from backup"
        );

        Ok(backup_path)
    }

    /// Finds the best backup for the target version.
    ///
    /// Prefers an exact version match. Otherwise, returns the newest backup whose
    /// version is less than or equal to the target version.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn find_best_backup(
        &self,
        target_version: &GitVersion,
    ) -> Result<Option<BackupEntry>, Error> {
        let manifest = self.read_manifest()?;

        let current_hash = current_migration_hash();
        let candidates: Vec<_> = manifest
            .entries
            .into_iter()
            .filter(|e| e.version <= *target_version)
            .filter(|e| {
                let compatible = e.migration_hash == current_hash;
                if !compatible {
                    tracing::warn!(
                        version = %e.version,
                        file = %e.file,
                        backup_migration_hash = %e.migration_hash,
                        current_migration_hash = %current_hash,
                        "skipping backup with incompatible migration hash"
                    );
                }
                compatible
            })
            .filter(|e| {
                let exists = self.backup_dir().join(&e.file).exists();
                if !exists {
                    tracing::warn!(
                        version = %e.version,
                        file = %e.file,
                        "skipping manifest entry whose backup file is missing"
                    );
                }
                exists
            })
            .collect();

        if candidates.is_empty() {
            return Ok(None);
        }

        let best = candidates
            .into_iter()
            .max_by(|a, b| a.version.cmp(&b.version))
            .expect("candidates should not be empty after filtering");

        Ok(Some(best))
    }

    /// Reads the backup manifest from disk.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    fn read_manifest(&self) -> Result<BackupManifest, Error> {
        if self.manifest_path().exists() {
            crate::helpers::load_toml::<BackupManifest, _>(&self.manifest_path())
                .context("failed to read backup manifest")
        } else {
            Ok(BackupManifest::default())
        }
    }

    /// Writes the backup manifest to disk.
    fn write_manifest(&self, manifest: &BackupManifest) -> Result<(), Error> {
        crate::helpers::save_toml(manifest, self.manifest_path())
            .context("failed to write backup manifest")
    }

    /// Updates the manifest with a new backup entry and removes old backups.
    ///
    /// The current version's entry is always retained. Old backups from other
    /// versions are removed until the total count is within `retention`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    async fn update_manifest_and_cleanup(
        &self,
        filename: &str,
        created_at: DateTime<Utc>,
        migration_hash: MigrationHash,
        retention: usize,
    ) -> Result<(), Error> {
        let mut manifest = self.read_manifest()?;

        manifest
            .entries
            .retain(|e| e.version != self.current_version);

        let new_entry = BackupEntry {
            version: self.current_version.clone(),
            file: filename.to_string(),
            created_at,
            migration_hash,
        };

        manifest.entries.push(new_entry);

        if manifest.entries.len() > retention {
            let (current, mut others): (Vec<_>, Vec<_>) = manifest
                .entries
                .drain(..)
                .partition(|e| e.version == self.current_version);

            others.sort_by(|a, b| a.version.cmp(&b.version));

            let max_others = retention.saturating_sub(current.len());
            let entries_to_remove = others.len().saturating_sub(max_others);
            let candidates: Vec<_> = others.drain(..entries_to_remove).collect();

            let mut failed: Vec<BackupEntry> = Vec::new();

            for entry in candidates {
                let file_path = self.backup_dir().join(&entry.file);

                if file_path.exists()
                    && let Err(e) = remove_sqlite_files(&file_path).await
                {
                    tracing::warn!(
                        version = %entry.version,
                        file = %entry.file,
                        error = %e,
                        "failed to remove old database backup, will retry on next cleanup"
                    );
                    failed.push(entry);
                    continue;
                }

                tracing::debug!(
                    version = %entry.version,
                    file = %entry.file,
                    "removed old database backup"
                );
            }

            others.extend(failed);
            manifest.entries = others;
            manifest.entries.extend(current);
        }

        self.write_manifest(&manifest)
    }
}

/// Renames an SQLite database file and its WAL/SHM companions.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(src, dest)))]
async fn rename_sqlite_files(src: &Path, dest: &Path) -> Result<(), Error> {
    fs::rename(src, dest)
        .await
        .with_context(|| format!("failed to rename {} to {}", src.display(), dest.display()))?;

    for suffix in SQLITE_COMPANION_SUFFIXES {
        let src_extra = add_suffix(src, suffix);
        if src_extra.exists() {
            let dest_extra = add_suffix(dest, suffix);
            fs::rename(&src_extra, &dest_extra).await.with_context(|| {
                format!(
                    "failed to rename {} to {}",
                    src_extra.display(),
                    dest_extra.display()
                )
            })?;
        }
    }

    Ok(())
}

/// Copies an SQLite database file and its WAL/SHM companions.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(src, dest)))]
async fn copy_sqlite_files(src: &Path, dest: &Path) -> Result<(), Error> {
    fs::copy(src, dest)
        .await
        .with_context(|| format!("failed to copy {} to {}", src.display(), dest.display()))?;

    for suffix in SQLITE_COMPANION_SUFFIXES {
        let src_extra = add_suffix(src, suffix);
        if src_extra.exists() {
            let dest_extra = add_suffix(dest, suffix);
            fs::copy(&src_extra, &dest_extra).await.with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    src_extra.display(),
                    dest_extra.display()
                )
            })?;
        }
    }

    Ok(())
}

/// Removes an SQLite database file and its WAL/SHM companions.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(path)))]
async fn remove_sqlite_files(path: &Path) -> Result<(), Error> {
    if path.exists() {
        fs::remove_file(path)
            .await
            .with_context(|| format!("failed to remove {}", path.display()))?;
    }

    for suffix in SQLITE_COMPANION_SUFFIXES {
        let extra = add_suffix(path, suffix);
        if extra.exists() {
            fs::remove_file(&extra)
                .await
                .with_context(|| format!("failed to remove {}", extra.display()))?;
        }
    }

    Ok(())
}

/// Adds a suffix to a path before the file extension.
fn add_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::runtime::RUNTIME;
    use sqlx::sqlite::SqlitePool;
    use std::str::FromStr;

    #[test]
    fn test_add_suffix() {
        let path = PathBuf::from("/tmp/cadmus.sqlite");
        assert_eq!(
            add_suffix(&path, "-wal"),
            PathBuf::from("/tmp/cadmus.sqlite-wal")
        );
    }

    #[test]
    fn test_create_backup_writes_file_and_manifest() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = dir.path().join("cadmus.sqlite");
        let mut db = Database::new(&db_path).expect("failed to create database");
        db.init_for_test(0).expect("failed to run migrations");

        let version = GitVersion::from_str("v0.10.0").unwrap();
        let manager = DbBackupManager::new(dir.path().to_path_buf(), version.clone());

        let backup_path = RUNTIME
            .block_on(async { manager.create_backup(db.pool(), &db_path, 2).await.unwrap() });

        assert!(backup_path.exists(), "backup file should exist");

        let manifest = manager.read_manifest().expect("failed to read manifest");
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].version, version);
        assert_eq!(manifest.entries[0].migration_hash, current_migration_hash());
        assert!(manifest.entries[0].file.contains("v0.10.0"));
    }

    #[test]
    fn test_backup_retention_removes_oldest_backups() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = dir.path().join("cadmus.sqlite");
        let mut db = Database::new(&db_path).expect("failed to create database");
        db.init_for_test(0).expect("failed to run migrations");

        let v1 = GitVersion::from_str("v0.9.0").unwrap();
        let v2 = GitVersion::from_str("v0.10.0").unwrap();
        let v3 = GitVersion::from_str("v0.11.0").unwrap();

        RUNTIME.block_on(async {
            let manager = DbBackupManager::new(dir.path().to_path_buf(), v1.clone());
            manager.create_backup(db.pool(), &db_path, 2).await.unwrap();

            let manager = DbBackupManager::new(dir.path().to_path_buf(), v2.clone());
            manager.create_backup(db.pool(), &db_path, 2).await.unwrap();

            let manager = DbBackupManager::new(dir.path().to_path_buf(), v3.clone());
            manager.create_backup(db.pool(), &db_path, 2).await.unwrap();
        });

        let manager = DbBackupManager::new(dir.path().to_path_buf(), v3.clone());
        let manifest = manager.read_manifest().expect("failed to read manifest");
        assert_eq!(manifest.entries.len(), 2);
        assert!(
            manifest.entries.iter().any(|e| e.version == v2),
            "v2 should be retained"
        );
        assert!(
            manifest.entries.iter().any(|e| e.version == v3),
            "v3 (current) should be retained"
        );
    }

    #[test]
    fn test_restore_best_backup_replaces_active_database() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = dir.path().join("cadmus.sqlite");
        let mut db = Database::new(&db_path).expect("failed to create database");
        db.init_for_test(0).expect("failed to run migrations");

        let older_version = GitVersion::from_str("v0.9.0").unwrap();
        let newer_version = GitVersion::from_str("v0.10.0").unwrap();

        let backup_path = RUNTIME.block_on(async {
            let manager = DbBackupManager::new(dir.path().to_path_buf(), older_version.clone());
            manager.create_backup(db.pool(), &db_path, 2).await.unwrap()
        });

        // Simulate a newer database by stamping it and closing it.
        let migration_hash = crate::db::version::current_migration_hash();
        RUNTIME.block_on(async {
            crate::db::version::stamp_db_version(db.pool(), &newer_version, &migration_hash)
                .await
                .unwrap();
        });
        db.close();

        let restored = RUNTIME.block_on(async {
            let manager = DbBackupManager::new(dir.path().to_path_buf(), older_version.clone());
            manager
                .restore_best_backup(&db_path, &newer_version)
                .await
                .expect("restore failed")
        });
        assert_eq!(restored, backup_path, "should restore the older backup");

        // Reopen and verify the restored database does not have the newer version stamp.
        let db = Database::new(&db_path).expect("failed to reopen database");
        let stored_version = RUNTIME.block_on(async {
            crate::db::version::read_db_version(db.pool())
                .await
                .unwrap()
        });
        assert_ne!(
            stored_version.as_ref(),
            Some(&newer_version),
            "restored database should not retain the newer version stamp"
        );
    }

    #[test]
    fn test_fs_copy_backup_preserves_data() {
        let dir = tempfile::Builder::new()
            .prefix("cadmus-backup-fs-")
            .tempdir()
            .expect("failed to create temp dir");

        let db_path = dir.path().join("cadmus.sqlite");
        let mut db = Database::new(&db_path).expect("failed to create database");
        db.init_for_test(0).expect("failed to run migrations");

        let test_version = GitVersion::from_str("v1.2.3").unwrap();
        let migration_hash = crate::db::version::current_migration_hash();

        let backup_path = RUNTIME.block_on(async {
            crate::db::version::stamp_db_version(db.pool(), &test_version, &migration_hash)
                .await
                .expect("failed to stamp test version");

            let manager = DbBackupManager::new(dir.path().to_path_buf(), test_version.clone());
            manager
                .create_backup(db.pool(), &db_path, 2)
                .await
                .expect("fs copy backup failed")
        });

        RUNTIME.block_on(async {
            let backup_url = format!("sqlite://{}", backup_path.display());
            let backup_pool = SqlitePool::connect(&backup_url)
                .await
                .expect("failed to open backup database");

            let version = crate::db::version::read_db_version(&backup_pool)
                .await
                .expect("failed to query backup")
                .expect("backup should have a version stamp");

            assert_eq!(
                version, test_version,
                "backup should contain the stamped version"
            );

            backup_pool.close().await;
        });
    }

    #[test]
    fn test_find_best_backup_skips_missing_files() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = dir.path().join("cadmus.sqlite");
        let mut db = Database::new(&db_path).expect("failed to create database");
        db.init_for_test(0).expect("failed to run migrations");

        let v090 = GitVersion::from_str("v0.9.0").unwrap();
        let v095 = GitVersion::from_str("v0.9.5").unwrap();
        let v100 = GitVersion::from_str("v0.10.0").unwrap();

        RUNTIME.block_on(async {
            let manager = DbBackupManager::new(dir.path().to_path_buf(), v090.clone());
            manager
                .create_backup(db.pool(), &db_path, 10)
                .await
                .unwrap();

            let manager = DbBackupManager::new(dir.path().to_path_buf(), v095.clone());
            manager
                .create_backup(db.pool(), &db_path, 10)
                .await
                .unwrap();
        });

        // Manually delete the v0.9.5 backup file, leaving its manifest entry
        // intact — this simulates a previously failed cleanup.
        let stale_file = dir
            .path()
            .join("backups")
            .join(format!("cadmus-{}.sqlite", v095));
        std::fs::remove_file(&stale_file).expect("failed to remove stale backup file");

        // find_best_backup called with v1.0.0 as the target should skip the
        // stale v0.9.5 entry and return the valid v0.9.0 backup instead.
        let manager = DbBackupManager::new(dir.path().to_path_buf(), v100.clone());
        let best = manager
            .find_best_backup(&v100)
            .expect("find_best_backup failed")
            .expect("should find a valid backup");
        assert_eq!(
            best.version, v090,
            "should skip the stale v0.9.5 entry and return v0.9.0"
        );
    }

    #[test]
    fn test_find_best_backup_selects_closest_older_version() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = dir.path().join("cadmus.sqlite");
        let mut db = Database::new(&db_path).expect("failed to create database");
        db.init_for_test(0).expect("failed to run migrations");

        let v090 = GitVersion::from_str("v0.9.0").unwrap();
        let v095 = GitVersion::from_str("v0.9.5").unwrap();
        let v100 = GitVersion::from_str("v0.10.0").unwrap();

        RUNTIME.block_on(async {
            let manager = DbBackupManager::new(dir.path().to_path_buf(), v090.clone());
            manager.create_backup(db.pool(), &db_path, 2).await.unwrap();

            let manager = DbBackupManager::new(dir.path().to_path_buf(), v100.clone());
            manager.create_backup(db.pool(), &db_path, 2).await.unwrap();
        });

        let manager = DbBackupManager::new(dir.path().to_path_buf(), v095.clone());
        let best = manager
            .find_best_backup(&v095)
            .expect("find_best_backup failed")
            .expect("should find a backup");
        assert_eq!(best.version, v090, "v0.9.0 is the closest older backup");
    }
}
