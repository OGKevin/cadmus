//! One-time migration of dynamic data files from the install directory to the
//! SD card data directory.
//!
//! This migration runs once at startup when a device with removable storage has
//! an SD card mounted. It moves settings, logs, and dictionaries — but **not**
//! the SQLite database, which is already open by the time migrations run.
//!
//! # Idempotency
//!
//! A directory is skipped if the source no longer exists. A file is skipped if
//! the destination already exists. Re-running the migration after a partial
//! failure is safe.

use std::fs;
use std::path::{Path, PathBuf};

/// Directories and individual files to migrate from the install dir to the data
/// dir. Dictionaries, settings, and logs are included; the SQLite database is
/// excluded because it is already open when this runs.
const MIGRATE_DIRS: &[&str] = &["Settings", "logs", "dictionaries"];
const MIGRATE_FILES: &[&str] = &["Settings.toml"];

crate::migration!(
    /// Migrates dynamic data files from the install directory to the SD card
    /// data directory on devices with removable storage.
    ///
    /// Settings, logs, and dictionaries are moved. The SQLite database is
    /// excluded because it is already open when migrations run.
    ///
    /// This migration is a no-op when no SD card is present (`data_dir` equals
    /// `install_dir`). It will be recorded as succeeded so it does not re-run
    /// on subsequent boots without a card.
    "v3_migrate_data_to_sd_card",
    async fn migrate_data_to_sd_card(_pool: &sqlx::SqlitePool) {
        let install_dir = crate::device::CURRENT_DEVICE.install_dir();
        let data_dir = crate::device::CURRENT_DEVICE.data_dir();

        migrate_data_to_sd(install_dir, data_dir);

        Ok(())
    }
);

fn migrate_data_to_sd(install_dir: PathBuf, data_dir: PathBuf) {
    if install_dir == data_dir {
        return;
    }

    tracing::info!(
        from = %install_dir.display(),
        to = %data_dir.display(),
        "migrating dynamic data files to sd card"
    );

    if let Err(e) = fs::create_dir_all(&data_dir) {
        tracing::warn!(
            path = %data_dir.display(),
            error = %e,
            "failed to create data dir on sd card; skipping migration"
        );
        return;
    }

    for dirname in MIGRATE_DIRS {
        migrate_dir(&install_dir.join(dirname), &data_dir.join(dirname));
    }

    for filename in MIGRATE_FILES {
        migrate_file(&install_dir.join(filename), &data_dir.join(filename));
    }
}

fn migrate_dir(src: &Path, dst: &Path) {
    if !src.exists() {
        return;
    }

    if let Err(e) = fs::create_dir_all(dst) {
        tracing::warn!(
            path = %dst.display(),
            error = %e,
            "failed to create destination dir; skipping directory migration"
        );
        return;
    }

    copy_dir_recursive(src, dst, src);

    if let Err(e) = fs::remove_dir_all(src) {
        tracing::warn!(
            path = %src.display(),
            error = %e,
            "failed to remove source directory after migration"
        );
    }
}

fn copy_dir_recursive(src_root: &Path, dst_root: &Path, current: &Path) {
    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(
                path = %current.display(),
                error = %e,
                "failed to read directory during migration"
            );
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "failed to read directory entry during migration");
                continue;
            }
        };

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                tracing::warn!(error = %e, "failed to get file type during migration");
                continue;
            }
        };

        let rel = match entry.path().strip_prefix(src_root) {
            Ok(r) => r.to_path_buf(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to strip prefix during migration");
                continue;
            }
        };

        let dst_path = dst_root.join(&rel);

        if file_type.is_dir() {
            if let Err(e) = fs::create_dir_all(&dst_path) {
                tracing::warn!(
                    path = %dst_path.display(),
                    error = %e,
                    "failed to create subdirectory during migration"
                );
                continue;
            }

            copy_dir_recursive(src_root, dst_root, &entry.path());
            continue;
        }

        if dst_path.exists() {
            continue;
        }

        migrate_file(&entry.path(), &dst_path);
    }
}

fn migrate_file(src: &Path, dst: &Path) {
    if !src.exists() || dst.exists() {
        return;
    }

    if let Err(e) = fs::copy(src, dst) {
        tracing::warn!(
            src = %src.display(),
            dst = %dst.display(),
            error = %e,
            "failed to copy file during migration"
        );
        return;
    }

    if let Err(e) = fs::remove_file(src) {
        tracing::warn!(
            path = %src.display(),
            error = %e,
            "failed to remove source file after migration"
        );
    }

    tracing::debug!(path = %src.display(), "migrated file to sd card");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn run(install: &TempDir, data: &TempDir) {
        migrate_data_to_sd(install.path().to_path_buf(), data.path().to_path_buf());
    }

    #[test]
    fn test_no_op_when_dirs_equal() {
        let dir = TempDir::new().unwrap();
        let settings = dir.path().join("Settings.toml");
        fs::write(&settings, "key = true").unwrap();

        migrate_data_to_sd(dir.path().to_path_buf(), dir.path().to_path_buf());

        assert!(
            settings.exists(),
            "file must be untouched when dirs are equal"
        );
    }

    #[test]
    fn test_migrates_top_level_file() {
        let install = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();

        fs::write(install.path().join("Settings.toml"), "key = true").unwrap();

        run(&install, &data);

        assert!(
            data.path().join("Settings.toml").exists(),
            "Settings.toml must exist in data dir"
        );
        assert!(
            !install.path().join("Settings.toml").exists(),
            "Settings.toml must be removed from install dir"
        );
    }

    #[test]
    fn test_migrates_directory_recursively() {
        let install = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();

        let src_dir = install.path().join("Settings");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("config.toml"), "x = 1").unwrap();

        let nested = src_dir.join("profiles");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("default.toml"), "y = 2").unwrap();

        run(&install, &data);

        assert!(data.path().join("Settings/config.toml").exists());
        assert!(data.path().join("Settings/profiles/default.toml").exists());
        assert!(
            !install.path().join("Settings").exists(),
            "source dir must be removed"
        );
    }

    #[test]
    fn test_idempotent_file() {
        let install = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();

        fs::write(install.path().join("Settings.toml"), "key = true").unwrap();

        run(&install, &data);

        fs::write(install.path().join("Settings.toml"), "key = false").unwrap();

        run(&install, &data);

        let content = fs::read_to_string(data.path().join("Settings.toml")).unwrap();
        assert_eq!(
            content, "key = true",
            "second run must not overwrite already-migrated file"
        );
    }

    #[test]
    fn test_idempotent_dir() {
        let install = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();

        let src_dir = install.path().join("Settings");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("a.toml"), "original").unwrap();

        run(&install, &data);

        fs::create_dir(install.path().join("Settings")).unwrap();
        fs::write(install.path().join("Settings/a.toml"), "overwrite").unwrap();

        run(&install, &data);

        let content = fs::read_to_string(data.path().join("Settings/a.toml")).unwrap();
        assert_eq!(
            content, "original",
            "second run must not overwrite already-migrated file"
        );
    }

    #[test]
    fn test_missing_source_dir_is_skipped() {
        let install = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();

        run(&install, &data);

        assert!(
            data.path().read_dir().unwrap().next().is_none(),
            "data dir must remain empty"
        );
    }
}
