//! Database access layer for monolingual dictionary metadata.
//!
//! Manages the `reader_dict_monolingual_metadata` table, which caches the
//! API response from `https://www.reader-dict.com/api/v1/dictionaries`.
//! Only monolingual entries (source language == target language) are stored.
//!
//! Also manages the `reader_dict_monolingual_installed` table, which tracks which version
//! of each dictionary is currently installed on the device.

use super::metadata::DictionaryEntry;
use crate::db::runtime::RUNTIME;
use crate::db::types::UnixTimestamp;
use crate::db::Database;
use anyhow::Error;
use sqlx::SqlitePool;

/// Database handle for monolingual dictionary tables.
#[derive(Clone, Debug)]
pub(super) struct Db {
    pool: SqlitePool,
}

impl Db {
    pub(super) fn new(database: &Database) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    /// Inserts or replaces a single monolingual metadata entry.
    ///
    /// The `updated` date is stored as a Unix epoch integer (midnight UTC).
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, entry), fields(lang = %lang)))]
    pub(super) fn upsert_entry(&self, lang: &str, entry: &DictionaryEntry) -> Result<(), Error> {
        let updated: UnixTimestamp = entry.updated.into();
        let cached_at = UnixTimestamp::now();
        let formats = &entry.formats;
        let words = entry.words as i64;

        RUNTIME.block_on(async {
            sqlx::query!(
                r#"INSERT INTO reader_dict_monolingual_metadata
                       (lang, formats, updated, words, cached_at)
                   VALUES (?, ?, ?, ?, ?)
                   ON CONFLICT(lang) DO UPDATE SET
                       formats   = excluded.formats,
                       updated   = excluded.updated,
                       words     = excluded.words,
                       cached_at = excluded.cached_at"#,
                lang,
                formats,
                updated,
                words,
                cached_at,
            )
            .execute(&self.pool)
            .await?;

            tracing::debug!(lang, "upserted monolingual metadata entry");
            Ok(())
        })
    }

    /// Retrieves the cached metadata entry for a single language.
    ///
    /// Returns `None` if no entry for `lang` has been cached yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(lang = %lang)))]
    pub(super) fn get_entry(&self, lang: &str) -> Result<Option<DictionaryEntry>, Error> {
        RUNTIME.block_on(async {
            let row = sqlx::query!(
                r#"SELECT formats, updated as "updated: UnixTimestamp", words
                   FROM reader_dict_monolingual_metadata
                   WHERE lang = ?"#,
                lang,
            )
            .fetch_optional(&self.pool)
            .await?;

            Ok(row.map(|r| DictionaryEntry {
                formats: r.formats,
                updated: r.updated.into(),
                words: r.words as u64,
            }))
        })
    }

    /// Retrieves all cached monolingual metadata entries.
    ///
    /// Returns an empty `Vec` if no entries have been cached yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails or any stored `updated`
    /// timestamp cannot be converted to a `NaiveDate`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub(super) fn get_all_entries(&self) -> Result<Vec<(String, DictionaryEntry)>, Error> {
        RUNTIME.block_on(async {
            let rows = sqlx::query!(
                r#"SELECT lang, formats, updated as "updated: UnixTimestamp", words
                   FROM reader_dict_monolingual_metadata"#,
            )
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter()
                .map(|r| {
                    Ok((
                        r.lang,
                        DictionaryEntry {
                            formats: r.formats,
                            updated: r.updated.into(),
                            words: r.words as u64,
                        },
                    ))
                })
                .collect()
        })
    }

    /// Returns the most recent `cached_at` value across all metadata entries.
    ///
    /// Used to determine whether the metadata cache is stale relative to
    /// the API's `Last-Modified` header.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub(super) fn get_most_recent_cached_at(&self) -> Result<Option<UnixTimestamp>, Error> {
        RUNTIME.block_on(async {
            let result = sqlx::query_scalar!(
                r#"SELECT MAX(cached_at) as "cached_at: UnixTimestamp"
                   FROM reader_dict_monolingual_metadata"#
            )
            .fetch_one(&self.pool)
            .await?;

            Ok(result)
        })
    }

    /// Records that a dictionary was installed with the given version.
    ///
    /// If a record already exists for `lang`, it is updated in place.
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(lang = %lang)))]
    pub(super) fn record_install(
        &self,
        lang: &str,
        installed_version: UnixTimestamp,
    ) -> Result<(), Error> {
        let installed_at = UnixTimestamp::now();

        RUNTIME.block_on(async {
            sqlx::query!(
                r#"INSERT INTO reader_dict_monolingual_installed (lang, installed_at, installed_version)
                   VALUES (?, ?, ?)
                   ON CONFLICT(lang) DO UPDATE SET
                       installed_at      = excluded.installed_at,
                       installed_version = excluded.installed_version"#,
                lang,
                installed_at,
                installed_version,
            )
            .execute(&self.pool)
            .await?;

            tracing::debug!(lang, "recorded dictionary install");
            Ok(())
        })
    }

    /// Removes the installed record for a language.
    ///
    /// Called when a dictionary is deleted from the device.
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(lang = %lang)))]
    pub(super) fn remove_installed(&self, lang: &str) -> Result<(), Error> {
        RUNTIME.block_on(async {
            sqlx::query!(
                r#"DELETE FROM reader_dict_monolingual_installed WHERE lang = ?"#,
                lang
            )
            .execute(&self.pool)
            .await?;

            tracing::debug!(lang, "removed dictionary installed record");
            Ok(())
        })
    }

    /// Returns `true` if a newer version of the dictionary is available.
    ///
    /// Compares `updated` from `reader_dict_monolingual_metadata` against
    /// `installed_version` from `reader_dict_monolingual_installed` via a single SQL JOIN.
    /// Returns `false` if the language is not installed or has no metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(lang = %lang), ret(level=tracing::Level::TRACE)))]
    pub(super) fn is_update_available(&self, lang: &str) -> Result<bool, Error> {
        RUNTIME.block_on(async {
            let result = sqlx::query_scalar!(
                r#"SELECT EXISTS(
                    SELECT 1
                    FROM reader_dict_monolingual_metadata m
                    JOIN reader_dict_monolingual_installed i ON m.lang = i.lang
                    WHERE m.lang = ? AND m.updated > i.installed_version
                ) as "exists: bool""#,
                lang
            )
            .fetch_one(&self.pool)
            .await?;

            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn create_test_db() -> (Database, Db) {
        let database = Database::new(":memory:").expect("failed to create in-memory database");
        database.migrate().expect("failed to run migrations");
        let db = Db::new(&database);
        (database, db)
    }

    fn make_entry(year: i32, month: u32, day: u32, words: u64) -> DictionaryEntry {
        DictionaryEntry {
            formats: "df,dic,dictorg,kobo,mobi,stardict".to_string(),
            updated: NaiveDate::from_ymd_opt(year, month, day).unwrap(),
            words,
        }
    }

    #[test]
    fn test_upsert_and_get_roundtrip() {
        let (_database, db) = create_test_db();
        let entry = make_entry(2026, 4, 1, 1_381_375);

        db.upsert_entry("en", &entry)
            .expect("upsert should succeed");

        let all = db.get_all_entries().expect("get_all should not fail");
        assert_eq!(all.len(), 1);
        let (lang, fetched) = &all[0];
        assert_eq!(lang, "en");
        assert_eq!(fetched.formats, entry.formats);
        assert_eq!(
            fetched.updated,
            NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
        );
        assert_eq!(fetched.words, 1_381_375);
    }

    #[test]
    fn test_upsert_overwrites_existing_entry() {
        let (_database, db) = create_test_db();

        db.upsert_entry("en", &make_entry(2026, 1, 1, 100))
            .expect("upsert should succeed");
        db.upsert_entry("en", &make_entry(2026, 4, 1, 1_381_375))
            .expect("upsert should succeed");

        let all = db.get_all_entries().expect("get_all should not fail");
        assert_eq!(all.len(), 1);
        let (_, fetched) = &all[0];
        assert_eq!(
            fetched.updated,
            NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
        );
        assert_eq!(fetched.words, 1_381_375);
    }

    #[test]
    fn test_get_all_entries_returns_all() {
        let (_database, db) = create_test_db();

        db.upsert_entry("en", &make_entry(2026, 4, 1, 1_381_375))
            .expect("upsert should succeed");
        db.upsert_entry("fr", &make_entry(2026, 3, 1, 2_050_655))
            .expect("upsert should succeed");

        let all = db.get_all_entries().expect("get_all should not fail");
        assert_eq!(all.len(), 2);

        let langs: Vec<&str> = all.iter().map(|(l, _)| l.as_str()).collect();
        assert!(langs.contains(&"en"));
        assert!(langs.contains(&"fr"));
    }

    #[test]
    fn test_get_all_entries_empty() {
        let (_database, db) = create_test_db();
        let all = db.get_all_entries().expect("get_all should not fail");
        assert!(all.is_empty());
    }

    #[test]
    fn test_get_most_recent_cached_at_empty() {
        let (_database, db) = create_test_db();
        let result = db
            .get_most_recent_cached_at()
            .expect("should not error on empty table");
        assert!(result.is_none());
    }

    #[test]
    fn test_get_most_recent_cached_at_returns_max() {
        let (_database, db) = create_test_db();

        db.upsert_entry("en", &make_entry(2026, 4, 1, 1_381_375))
            .expect("upsert should succeed");
        db.upsert_entry("fr", &make_entry(2026, 3, 1, 2_050_655))
            .expect("upsert should succeed");

        let result = db.get_most_recent_cached_at().expect("should not error");
        assert!(result.is_some());
    }

    #[test]
    fn test_remove_installed() {
        let (_database, db) = create_test_db();
        let version = UnixTimestamp::now();

        db.record_install("en", version)
            .expect("record_install should succeed");
        db.remove_installed("en")
            .expect("remove_installed should succeed");

        let result = db.is_update_available("en").expect("should not error");
        assert!(!result);
    }

    #[test]
    fn test_is_update_available_no_update() {
        let (_database, db) = create_test_db();

        let date = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        let version: UnixTimestamp = date.into();

        db.upsert_entry("en", &make_entry(2026, 4, 1, 1_381_375))
            .expect("upsert should succeed");
        db.record_install("en", version)
            .expect("record_install should succeed");

        let result = db.is_update_available("en").expect("should not error");
        assert!(!result);
    }

    #[test]
    fn test_is_update_available_with_update() {
        let (_database, db) = create_test_db();

        let old_version: UnixTimestamp = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap().into();

        db.upsert_entry("en", &make_entry(2026, 4, 1, 1_381_375))
            .expect("upsert should succeed");
        db.record_install("en", old_version)
            .expect("record_install should succeed");

        let result = db.is_update_available("en").expect("should not error");
        assert!(result);
    }

    #[test]
    fn test_is_update_available_not_installed() {
        let (_database, db) = create_test_db();

        db.upsert_entry("en", &make_entry(2026, 4, 1, 1_381_375))
            .expect("upsert should succeed");

        let result = db.is_update_available("en").expect("should not error");
        assert!(!result);
    }
}
