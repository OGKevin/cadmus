//! Database access layer for monolingual dictionary metadata.
//!
//! Manages the `dictionary_monolingual_metadata` table, which caches the
//! API response from `https://www.reader-dict.com/api/v1/dictionaries`.
//! Only monolingual entries (source language == target language) are stored.

use super::metadata::DictionaryEntry;
use crate::db::runtime::RUNTIME;
use crate::db::types::UnixTimestamp;
use crate::db::Database;
use anyhow::Error;
use chrono::NaiveDate;
use sqlx::SqlitePool;

/// Database handle for `dictionary_monolingual_metadata`.
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
    /// The `updated` date string (e.g. `"2026-04-01"`) is parsed as midnight
    /// UTC and stored as a Unix epoch integer.
    ///
    /// # Errors
    ///
    /// Returns an error if the date string cannot be parsed or the database
    /// write fails.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, entry), fields(lang = %lang)))]
    pub(super) fn upsert_entry(&self, lang: &str, entry: &DictionaryEntry) -> Result<(), Error> {
        let updated = parse_date_to_timestamp(&entry.updated)?;
        let cached_at = UnixTimestamp::now();
        let formats = &entry.formats;
        let words = entry.words as i64;

        RUNTIME.block_on(async {
            sqlx::query!(
                r#"INSERT INTO dictionary_monolingual_metadata
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

    /// Retrieves all cached monolingual metadata entries.
    ///
    /// Returns an empty `Vec` if no entries have been cached yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails or any stored `updated`
    /// timestamp cannot be formatted as a date string.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub(super) fn get_all_entries(&self) -> Result<Vec<(String, DictionaryEntry)>, Error> {
        RUNTIME.block_on(async {
            let rows = sqlx::query!(
                r#"SELECT lang, formats, updated as "updated: UnixTimestamp", words
                   FROM dictionary_monolingual_metadata"#,
            )
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter()
                .map(|r| {
                    Ok((
                        r.lang,
                        DictionaryEntry {
                            formats: r.formats,
                            updated: format_timestamp_to_date(r.updated)?,
                            words: r.words as u64,
                        },
                    ))
                })
                .collect()
        })
    }
}

/// Parses an ISO 8601 date string (e.g. `"2026-04-01"`) to midnight UTC as a
/// `UnixTimestamp`.
fn parse_date_to_timestamp(date_str: &str) -> Result<UnixTimestamp, Error> {
    let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("invalid date '{}': {}", date_str, e))?;
    let dt: chrono::NaiveDateTime = date.and_hms_opt(0, 0, 0).unwrap();
    Ok(UnixTimestamp::from(dt))
}

/// Formats a `UnixTimestamp` back to an ISO 8601 date string (e.g. `"2026-04-01"`).
fn format_timestamp_to_date(ts: UnixTimestamp) -> Result<String, Error> {
    let dt: chrono::NaiveDateTime = ts.into();
    Ok(dt.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_db() -> (Database, Db) {
        let database = Database::new(":memory:").expect("failed to create in-memory database");
        database.migrate().expect("failed to run migrations");
        let db = Db::new(&database);
        (database, db)
    }

    fn make_entry(updated: &str, words: u64) -> DictionaryEntry {
        DictionaryEntry {
            formats: "df,dic,dictorg,kobo,mobi,stardict".to_string(),
            updated: updated.to_string(),
            words,
        }
    }

    #[test]
    fn test_upsert_and_get_roundtrip() {
        let (_database, db) = create_test_db();
        let entry = make_entry("2026-04-01", 1_381_375);

        db.upsert_entry("en", &entry)
            .expect("upsert should succeed");

        let all = db.get_all_entries().expect("get_all should not fail");
        assert_eq!(all.len(), 1);
        let (lang, fetched) = &all[0];
        assert_eq!(lang, "en");
        assert_eq!(fetched.formats, entry.formats);
        assert_eq!(fetched.updated, "2026-04-01");
        assert_eq!(fetched.words, 1_381_375);
    }

    #[test]
    fn test_upsert_overwrites_existing_entry() {
        let (_database, db) = create_test_db();

        db.upsert_entry("en", &make_entry("2026-01-01", 100))
            .expect("upsert should succeed");
        db.upsert_entry("en", &make_entry("2026-04-01", 1_381_375))
            .expect("upsert should succeed");

        let all = db.get_all_entries().expect("get_all should not fail");
        assert_eq!(all.len(), 1);
        let (_, fetched) = &all[0];
        assert_eq!(fetched.updated, "2026-04-01");
        assert_eq!(fetched.words, 1_381_375);
    }

    #[test]
    fn test_get_all_entries_returns_all() {
        let (_database, db) = create_test_db();

        db.upsert_entry("en", &make_entry("2026-04-01", 1_381_375))
            .expect("upsert should succeed");
        db.upsert_entry("fr", &make_entry("2026-03-01", 2_050_655))
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
    fn test_parse_date_to_timestamp_roundtrip() {
        let date_str = "2026-04-01";
        let ts = parse_date_to_timestamp(date_str).expect("parse should succeed");
        let formatted = format_timestamp_to_date(ts).expect("format should succeed");
        assert_eq!(formatted, date_str);
    }

    #[test]
    fn test_parse_date_invalid_returns_error() {
        assert!(parse_date_to_timestamp("not-a-date").is_err());
        assert!(parse_date_to_timestamp("2026/04/01").is_err());
    }
}
