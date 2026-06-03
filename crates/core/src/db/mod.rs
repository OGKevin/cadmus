pub mod migrations;
pub mod runtime;
pub mod types;

use anyhow::Error;
use runtime::RUNTIME;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;

/// The filename of the SQLite database used by Cadmus.
pub const DB_FILENAME: &str = "cadmus.sqlite";

/// Database handle providing synchronous API over async SQLx operations.
/// Uses a bridge pattern with `RUNTIME.block_on()` to maintain synchronous interface
/// for compatibility with existing single-threaded event loop.
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish()
    }
}

impl Database {
    /// Create a new database connection pool.
    ///
    /// Does not run any migrations — call [`Database::migrate`] after construction.
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
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let path_str = path.display().to_string();

        tracing::info!(db_path = %path_str, "connecting to database");

        RUNTIME.block_on(async {
            let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path_str))?
                .create_if_missing(true)
                .foreign_keys(true);

            let pool = SqlitePoolOptions::new()
                .max_connections(5)
                .connect_with(options)
                .await?;

            tracing::info!(db_path = %path_str, "database connected");
            Ok(Database { pool })
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

    /// Run all migrations: sqlx file migrations first, then runtime macro migrations.
    ///
    /// Must be called once after [`Database::new`] before the database is used.
    /// Intended for use in the synchronous startup path.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn migrate(&self) -> Result<(), Error> {
        RUNTIME.block_on(async {
            tracing::info!("running schema migrations");
            #[cfg(feature = "tracing")]
            let span = tracing::info_span!("sqlx_migrations").entered();
            sqlx::migrate!("./migrations").run(&self.pool).await?;
            #[cfg(feature = "tracing")]
            span.exit();

            tracing::info!("running runtime migrations");
            self.migration_runner().run_all().await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_creation() {
        let db = Database::new(":memory:").expect("failed to create in-memory database");
        db.migrate().expect("failed to run migrations");

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
}
