//! Rebuildable dictionary word index, stored beside the library database.

use std::path::Path;

use anyhow::Error;
use sqlx::SqlitePool;

use crate::db::SqliteFile;
use crate::db::backup::remove_sqlite_files;

/// SQLite file that stores the rebuildable dictionary word index.
///
/// Installed-dictionary rows stay in the library database. This file holds
/// only the word index, which the dictionary index task rebuilds from the
/// dictionary files on disk.
#[derive(Clone)]
pub struct DictionaryDb {
    file: SqliteFile,
}

impl DictionaryDb {
    /// File name of the word index, stored beside `cadmus.sqlite`.
    pub const FILENAME: &'static str = "dictionary.sqlite";

    /// Opens `path` and applies the dictionary migrations.
    ///
    /// On connect or migrate failure, deletes the file and its `-wal` and
    /// `-shm` companions, then opens the path once more. A second failure is
    /// returned so the caller can stop startup.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(fields(db_path = %path.as_ref().display()))
    )]
    pub async fn open<P: AsRef<Path> + std::fmt::Debug>(path: P) -> Result<Self, Error> {
        let path = path.as_ref();
        let migrator = sqlx::migrate!("./dictionary/migrations");
        match Self::connect_and_migrate(path, &migrator).await {
            Ok(db) => Ok(db),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "dictionary database open failed; deleting the file and retrying"
                );
                remove_sqlite_files(path).await?;
                Self::connect_and_migrate(path, &migrator).await
            }
        }
    }

    async fn connect_and_migrate(
        path: &Path,
        migrator: &sqlx::migrate::Migrator,
    ) -> Result<Self, Error> {
        let file = SqliteFile::connect(path).await?;
        if let Err(error) = file.run_migrations(migrator).await {
            file.close().await;
            return Err(error);
        }
        Ok(Self { file })
    }

    /// Borrow the pool so callers can query the word index.
    pub fn pool(&self) -> &SqlitePool {
        self.file.pool()
    }

    /// Close every connection so the process releases the file.
    pub async fn close(&self) {
        self.file.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::db::backup::DbBackupManager;
    use crate::version::get_current_version;

    async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
        let found: Option<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(name)
                .fetch_optional(pool)
                .await
                .expect("sqlite_master");
        found.is_some()
    }

    #[tokio::test]
    async fn corrupt_dictionary_file_is_replaced_and_left_out_of_library_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let library_path = dir.path().join(crate::db::DB_FILENAME);
        let dictionary_path = dir.path().join(DictionaryDb::FILENAME);

        let mut database = Database::new(&library_path)
            .await
            .expect("library database");
        database.init_for_test(0).await.expect("library init");

        tokio::fs::write(&dictionary_path, b"not a database")
            .await
            .expect("write garbage dictionary file");

        database
            .init_for_test(0)
            .await
            .expect("second library init");

        let garbage = tokio::fs::read(&dictionary_path)
            .await
            .expect("read dictionary file after library init");
        assert_eq!(
            garbage, b"not a database",
            "library quick_check must not open dictionary.sqlite"
        );
        let quick_check: Option<String> = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_one(database.pool())
            .await
            .expect("library quick_check");
        assert_eq!(quick_check.as_deref(), Some("ok"));

        let dictionary = DictionaryDb::open(&dictionary_path)
            .await
            .expect("replace corrupt dictionary database");

        assert!(
            table_exists(dictionary.pool(), "dictionary_index_meta").await,
            "dictionary pool must contain dictionary_index_meta"
        );
        assert!(
            !table_exists(database.pool(), "dictionary_index_meta").await,
            "library pool must not contain dictionary_index_meta"
        );
        let meta_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dictionary_index_meta")
            .fetch_one(dictionary.pool())
            .await
            .expect("count meta");
        let entry_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dictionary_index_entry")
            .fetch_one(dictionary.pool())
            .await
            .expect("count entries");
        assert_eq!(meta_rows, 0);
        assert_eq!(entry_rows, 0);

        let manager = DbBackupManager::new(dir.path().to_path_buf(), get_current_version());
        let backup_path = manager
            .create_backup(database.pool(), database.path(), 2)
            .await
            .expect("library backup");

        let backup_dir = dir.path().join("backups");
        let mut entries = tokio::fs::read_dir(&backup_dir)
            .await
            .expect("backup directory");
        while let Some(entry) = entries.next_entry().await.expect("backup entry") {
            assert_ne!(
                entry.file_name(),
                std::ffi::OsString::from(DictionaryDb::FILENAME),
                "backup directory must not contain dictionary.sqlite"
            );
        }

        let backup_pool = SqlitePool::connect(&format!("sqlite://{}", backup_path.display()))
            .await
            .expect("open backup");
        assert!(
            !table_exists(&backup_pool, "dictionary_index_entry").await,
            "library backup must not contain dictionary_index_entry"
        );
        backup_pool.close().await;
    }
}
