pub mod backup;
pub mod migrations;
pub mod runtime;
pub mod types;
pub mod version;

use anyhow::{Context, Error};
use log::LevelFilter;
use runtime::RUNTIME;
use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use crate::version::get_current_version;

/// The filename of the SQLite database used by Cadmus.
pub const DB_FILENAME: &str = "cadmus.sqlite";

/// Database handle providing synchronous API over async SQLx operations.
/// Uses a bridge pattern with `RUNTIME.block_on()` to maintain synchronous interface
/// for compatibility with existing single-threaded event loop.
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
    /// The path to the database file.
    db_path: PathBuf,
    /// The directory containing the database file.
    ///
    /// Will be empty if the database path is in-memory or has no parent directory.
    db_dir: Option<PathBuf>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish()
    }
}

impl Database {
    /// Create a new database connection pool.
    ///
    /// Does not run any migrations — call [`Database::init`] after construction.
    ///
    /// # Arguments
    /// * `path` - Path to the SQLite database file (will be created if it doesn't exist)
    ///
    /// # Returns
    /// * `Ok(Database)` - Successfully connected database
    /// * `Err(Error)` - Connection failure
    #[cfg_attr(feature = "tracing", tracing::instrument(fields(db_path = %path.as_ref().display())))]
    pub fn new<P: AsRef<Path> + std::fmt::Debug>(path: P) -> Result<Self, Error> {
        let path = path.as_ref();
        let db_dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf);

        if let Some(dir) = &db_dir {
            std::fs::create_dir_all(dir)?;
        }

        let path_str = path.display().to_string();

        tracing::info!(db_path = %path_str, "connecting to database");

        RUNTIME.block_on(async {
            let pool = open_pool(path).await?;
            tracing::info!(db_path = %path_str, "database connected");
            Ok(Database {
                pool,
                db_path: path.to_path_buf(),
                db_dir,
            })
        })
    }

    /// Close all connections in the pool, checkpointing WAL and releasing file handles.
    ///
    /// After calling this, no further database operations should be performed.
    /// This must be called before unmounting the filesystem that contains the database file,
    /// to ensure SQLite releases all file descriptors and flushes any pending WAL data.
    pub fn close(&self) {
        tracing::info!("closing database connection pool");
        RUNTIME.block_on(async {
            self.pool.close().await;
        });
        tracing::info!("database connection pool closed");
    }

    /// Returns a reference to the SQLite connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Returns a `MigrationRunner` bound to this database's pool.
    ///
    /// Use this to execute all registered runtime migrations after the
    /// database is initialized.
    pub fn migration_runner(&self) -> migrations::MigrationRunner {
        migrations::MigrationRunner::new(self.pool.clone())
    }

    /// Initialises the database for use by the application.
    ///
    /// Performs, in order:
    /// 1. Integrity check (`PRAGMA quick_check`).
    /// 2. Version gate — detects upgrades, downgrades, and fresh installs.
    /// 3. Restore from backup if corruption or downgrade is detected.
    /// 4. Schema and runtime migrations.
    /// 5. Version stamp update.
    /// 6. Post-migration backup (when the version changes).
    ///
    /// Must be called once after [`Database::new`] before the database is used.
    /// Intended for use in the synchronous startup path.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn init(&mut self, backup_retention: usize) -> Result<(), Error> {
        let app_version = get_current_version();

        RUNTIME.block_on(async {
            self.restore_if_needed(&app_version).await?;
            self.migrate().await?;
            version::stamp_db_version(&self.pool, &app_version, &version::current_migration_hash())
                .await?;
            tracing::info!(app_version = %app_version, "database version stamped");
            self.create_version_backup(&app_version, backup_retention)
                .await?;
            Ok(())
        })
    }

    /// Checks integrity and the version gate, restoring a backup when either fails.
    ///
    /// Corruption and downgrade are treated identically: close the pool, restore
    /// the best available backup for `app_version`, and reopen the pool so the
    /// caller can continue with migrations against a known-good database.
    ///
    /// Returns an error if a restore is needed but no backup directory or
    /// compatible backup exists.
    ///
    /// # Invariant
    ///
    /// `db_path` is always a real file path at the point where the pool is
    /// reopened. This is guaranteed because `db_dir` is `None` only for
    /// `:memory:` databases, which return early before reaching that point.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    async fn restore_if_needed(
        &mut self,
        app_version: &crate::version::GitVersion,
    ) -> Result<(), Error> {
        tracing::info!("checking database integrity");
        let integrity = self.check_integrity().await;

        let gate = if integrity.is_ok() {
            version::check_version_gate(&self.pool, app_version).await?
        } else {
            version::VersionGateResult::Unknown
        };

        let needs_restore = integrity.is_err() || gate == version::VersionGateResult::Downgrade;

        if !needs_restore {
            log_version_gate(gate);
            return Ok(());
        }

        let Some(ref db_dir) = self.db_dir.clone() else {
            if let Err(e) = integrity {
                tracing::error!(
                    app_version = %app_version,
                    "database corruption detected but no database directory available for backup restore"
                );
                return Err(e);
            }
            tracing::error!(
                app_version = %app_version,
                "downgrade detected but no database directory available for backup restore"
            );
            return Err(Error::msg(
                "downgrade detected but no database directory available for backup restore",
            ));
        };

        let db_version = if integrity.is_ok() {
            version::read_db_version(&self.pool)
                .await?
                .unwrap_or_else(|| app_version.clone())
        } else {
            app_version.clone()
        };

        if integrity.is_err() {
            tracing::warn!(
                app_version = %app_version,
                "database corruption detected; attempting restore from backup"
            );
        } else {
            tracing::warn!(
                app_version = %app_version,
                db_version = %db_version,
                "database was touched by a newer Cadmus version; restoring backup"
            );
        }

        let backup_manager = backup::DbBackupManager::new(db_dir.clone(), app_version.clone());
        self.pool.close().await;

        let restore_context = if integrity.is_err() {
            "corruption detected but no compatible backup found to restore"
        } else {
            "downgrade detected but no compatible backup found to restore"
        };

        let backup_path = backup_manager
            .restore_best_backup(&self.db_path, &db_version)
            .await
            .map_err(|e| {
                tracing::error!(app_version = %app_version, error = %e, restore_context, "restore failed");
                Error::from(e).context(restore_context)
            })?;

        self.pool = open_pool(&self.db_path).await?;

        tracing::info!(backup_path = %backup_path.display(), "database restored from backup");

        Ok(())
    }

    /// Creates a versioned backup after migrations on every startup.
    ///
    /// Skipped in test builds, when the database is in-memory (no `db_dir`),
    /// or when `retention` is zero.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    async fn create_version_backup(
        &self,
        app_version: &crate::version::GitVersion,
        retention: usize,
    ) -> Result<(), Error> {
        let Some(ref db_dir) = self.db_dir else {
            return Ok(());
        };

        if cfg!(test) {
            return Ok(());
        }

        if retention == 0 {
            tracing::debug!("database backups disabled (db_backup_retention = 0)");
            return Ok(());
        }

        let backup_manager = backup::DbBackupManager::new(db_dir.clone(), app_version.clone());
        backup_manager.create_backup(&self.pool, retention).await?;

        Ok(())
    }

    /// Runs schema migrations (sqlx) followed by runtime migrations.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    async fn migrate(&mut self) -> Result<(), Error> {
        tracing::info!("running schema migrations");
        #[cfg(feature = "tracing")]
        let span = tracing::info_span!("sqlx_migrations").entered();
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        #[cfg(feature = "tracing")]
        span.exit();

        tracing::info!("running runtime migrations");
        self.migration_runner().run_all().await?;

        Ok(())
    }

    /// Runs a lightweight SQLite integrity check.
    ///
    /// Checkpoints the WAL first so that any WAL corruption is caught and all
    /// WAL pages are flushed into the main file before `PRAGMA quick_check`
    /// runs. Without this, a corrupt WAL would be invisible to `quick_check`.
    ///
    /// `PRAGMA wal_checkpoint` returns nullable integer columns (`busy`, `log`,
    /// `checkpointed`) that sqlx typed macros cannot map, so an untyped
    /// [`sqlx::query()`] with `.execute()` is used — only the success or failure
    /// of the checkpoint matters here.
    ///
    /// Returns `Ok(())` if both the checkpoint and `PRAGMA quick_check` succeed.
    /// On failure, logs the error and returns it so the caller can decide
    /// whether to restore.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    async fn check_integrity(&self) -> Result<(), Error> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
            .context("failed to run PRAGMA wal_checkpoint")?;

        let result: Option<String> = sqlx::query_scalar!("PRAGMA quick_check")
            .fetch_one(&self.pool)
            .await
            .context("failed to run PRAGMA quick_check")?;

        if result == Some("ok".to_string()) {
            tracing::info!("database integrity check passed");
            Ok(())
        } else {
            tracing::error!(result = ?result, "database integrity check failed");
            Err(Error::msg(format!(
                "database integrity check failed: {:?}",
                result
            )))
        }
    }
}

/// Logs the outcome of a version gate check that does not require a restore.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(gate)))]
fn log_version_gate(gate: version::VersionGateResult) {
    match gate {
        version::VersionGateResult::Upgrade => {
            tracing::info!("database is from an older Cadmus version; upgrading");
        }
        version::VersionGateResult::Unknown => {
            tracing::info!("no version stamp found in database; treating as fresh install");
        }
        version::VersionGateResult::Current => {
            tracing::info!("database version matches current app version");
        }
        version::VersionGateResult::CompatibleDowngrade => {
            tracing::info!(
                "database was written by a newer Cadmus version with matching migrations"
            );
        }
        version::VersionGateResult::Downgrade => unreachable!(),
    }
}

/// Opens a connection pool for the given SQLite database path.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(path)))]
async fn open_pool(path: &Path) -> Result<SqlitePool, Error> {
    let path_str = path.display().to_string();
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path_str))?
        .create_if_missing(true)
        .foreign_keys(true)
        .log_slow_statements(LevelFilter::Warn, Duration::from_secs(2));

    SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .context("failed to open database pool")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_creation() {
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        RUNTIME.block_on(async {
            let result: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='books'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("failed to query sqlite_master");

            assert_eq!(result.0, 1, "books table should exist after migrations");
        });
    }

    #[test]
    fn test_migrate_stamps_version_on_first_run() {
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");

        let version = RUNTIME.block_on(async { version::read_db_version(&db.pool).await.unwrap() });
        assert_eq!(
            version,
            Some(get_current_version()),
            "first migrate should stamp the database with the current app version"
        );
    }

    #[test]
    fn test_migrate_current_version_is_idempotent() {
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("first migrate");
        db.init(0)
            .expect("second migrate should succeed (Current path)");

        let version = RUNTIME.block_on(async { version::read_db_version(&db.pool).await.unwrap() });
        assert_eq!(version, Some(get_current_version()));
    }

    #[test]
    fn test_migrate_upgrade_from_older_version() {
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("initial migrate");

        let older = crate::version::GitVersion::parse("v0.0.1").unwrap();
        let migration_hash = version::current_migration_hash();
        RUNTIME.block_on(async {
            version::stamp_db_version(&db.pool, &older, &migration_hash)
                .await
                .unwrap();
        });

        db.init(0).expect("migrate should succeed (Upgrade path)");

        let version = RUNTIME.block_on(async { version::read_db_version(&db.pool).await.unwrap() });
        assert_eq!(
            version,
            Some(get_current_version()),
            "migrate should re-stamp with current version after upgrade"
        );
    }

    #[test]
    fn test_migrate_downgrade_without_db_dir_errors() {
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("initial migrate");

        let newer = crate::version::GitVersion::parse("v99.99.99").unwrap();
        let migration_hash = incompatible_migration_hash();
        RUNTIME.block_on(async {
            version::stamp_db_version(&db.pool, &newer, &migration_hash)
                .await
                .unwrap();
        });

        let err = db
            .init(0)
            .expect_err("init should fail on downgrade without db_dir");
        assert!(
            err.to_string().contains("no database directory available"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_migrate_downgrade_with_db_dir_errors_without_backup() {
        let dir = tempfile::Builder::new()
            .prefix("cadmus-downgrade-no-backup-")
            .tempdir()
            .expect("failed to create temp dir");
        let db_path = dir.path().join("test.sqlite");

        let mut db = Database::new(db_path.to_str().unwrap()).expect("failed to create database");
        db.init(0).expect("initial migrate");

        let newer = crate::version::GitVersion::parse("v99.99.99").unwrap();
        let migration_hash = incompatible_migration_hash();
        RUNTIME.block_on(async {
            version::stamp_db_version(&db.pool, &newer, &migration_hash)
                .await
                .unwrap();
        });

        let err = db
            .init(0)
            .expect_err("init should fail when no backup is available");
        assert!(
            err.to_string().contains("no compatible backup found"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_migrate_downgrade_with_db_dir_restores_backup() {
        let dir = tempfile::Builder::new()
            .prefix("cadmus-downgrade-restore-")
            .tempdir()
            .expect("failed to create temp dir");
        let db_path = dir.path().join("test.sqlite");

        let mut db = Database::new(db_path.to_str().unwrap()).expect("failed to create database");
        db.init(0).expect("initial migrate");

        let app_version = get_current_version();
        RUNTIME.block_on(async {
            let backup_manager =
                backup::DbBackupManager::new(dir.path().to_path_buf(), app_version.clone());
            backup_manager.create_backup(&db.pool, 2).await.unwrap();
        });

        let newer = crate::version::GitVersion::parse("v99.99.99").unwrap();
        let migration_hash = incompatible_migration_hash();
        RUNTIME.block_on(async {
            version::stamp_db_version(&db.pool, &newer, &migration_hash)
                .await
                .unwrap();
        });

        db.init(0)
            .expect("migrate should succeed on downgrade with db_dir (restore path)");

        let version = RUNTIME.block_on(async { version::read_db_version(&db.pool).await.unwrap() });
        assert_eq!(
            version,
            Some(get_current_version()),
            "migrate should re-stamp with current version after downgrade restore"
        );

        let demoted = dir
            .path()
            .join("backups")
            .join(format!("cadmus-{}-demoted.sqlite", newer));
        assert!(
            demoted.exists(),
            "demoted file should be named after the DB version ({}), not the app version",
            newer
        );
    }

    #[test]
    fn test_check_integrity_passes_on_valid_database() {
        let db = Database::new(":memory:").expect("failed to create in-memory database");
        RUNTIME.block_on(async {
            db.check_integrity()
                .await
                .expect("integrity check should pass on a fresh database");
        });
    }

    #[test]
    fn test_init_restores_backup_on_corruption() {
        let dir = tempfile::Builder::new()
            .prefix("cadmus-corruption-restore-")
            .tempdir()
            .expect("failed to create temp dir");
        let db_path = dir.path().join("cadmus.sqlite");

        let mut db = Database::new(db_path.to_str().unwrap()).expect("failed to create database");
        db.init(0).expect("failed to run initial migrations");

        let app_version = get_current_version();
        RUNTIME.block_on(async {
            let backup_manager =
                backup::DbBackupManager::new(dir.path().to_path_buf(), app_version.clone());
            backup_manager.create_backup(&db.pool, 2).await.unwrap();
        });

        RUNTIME.block_on(async { db.pool.close().await });

        {
            let mut bytes = std::fs::read(&db_path).expect("failed to read db file");
            for chunk in bytes[100..].chunks_mut(512) {
                chunk.fill(0xFF);
            }
            std::fs::write(&db_path, &bytes).expect("failed to write corrupted db");
        }

        let mut db = Database::new(db_path.to_str().unwrap()).expect("failed to reopen database");
        db.init(0)
            .expect("init should restore from backup on corruption");

        let version = RUNTIME.block_on(async { version::read_db_version(&db.pool).await.unwrap() });
        assert_eq!(
            version,
            Some(get_current_version()),
            "restored database should be stamped with current version"
        );
    }

    #[test]
    fn test_check_integrity_fails_on_corrupted_database() {
        let dir = tempfile::Builder::new()
            .prefix("cadmus-integrity-test-")
            .tempdir()
            .expect("failed to create temp dir");
        let db_path = dir.path().join("corrupt.sqlite");

        let mut db = Database::new(db_path.to_str().unwrap()).expect("failed to create database");
        db.init(0).expect("failed to run migrations");

        RUNTIME.block_on(async { db.pool.close().await });

        {
            let mut bytes = std::fs::read(&db_path).expect("failed to read db file");
            for chunk in bytes[100..].chunks_mut(512) {
                chunk.fill(0xFF);
            }
            std::fs::write(&db_path, &bytes).expect("failed to write corrupted db");
        }

        let db = Database::new(db_path.to_str().unwrap()).expect("failed to reopen database");
        let result = RUNTIME.block_on(async { db.check_integrity().await });
        let err = result.expect_err("integrity check should fail on corrupted database");
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("integrity check failed")
                || err_msg.contains("PRAGMA quick_check")
                || err_msg.contains("wal_checkpoint"),
            "expected integrity-related failure, got: {err_msg}"
        );
    }

    fn incompatible_migration_hash() -> version::MigrationHash {
        blake3::hash(uuid::Uuid::now_v7().as_bytes())
            .to_hex()
            .to_string()
            .parse()
            .unwrap()
    }
}
