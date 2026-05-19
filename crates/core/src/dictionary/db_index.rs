//! SQLite-backed dictionary index reader.
//!
//! Replaces the in-memory `.index` file reader with a database-backed implementation
//! that supports both single-dictionary and cross-dictionary word lookups.

use levenshtein::levenshtein;
use sqlx::SqlitePool;

use crate::db::runtime::RUNTIME;
use crate::db::Database;
use crate::helpers::Fp;

use super::indexing::{Entry, IndexReader};
use super::Metadata;

/// Escapes SQLite LIKE wildcards (`%`, `_`) and the escape character (`\`)
/// so a user-supplied prefix is matched literally.
fn escape_like_prefix(prefix: &str) -> String {
    prefix
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// SQLite-backed implementation of [`IndexReader`].
///
/// When `fingerprint` is `Some`, queries are scoped to that dictionary.
/// When `None`, queries search across all indexed dictionaries.
pub struct DbIndexReader {
    pool: SqlitePool,
    fingerprint: Option<Fp>,
}

impl DbIndexReader {
    /// Creates a new reader backed by `database`, optionally scoped to `fingerprint`.
    pub fn new(database: &Database, fingerprint: Option<Fp>) -> Self {
        Self {
            pool: database.pool().clone(),
            fingerprint,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword)))]
    async fn exact_scoped(&self, headword: &str, fp: &str) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word, offset, size, original
               FROM dictionary_index_entry
               WHERE fingerprint = ? AND word = ?"#,
            fp,
            headword,
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|r| Entry {
                    headword: r.word,
                    offset: r.offset as u64,
                    size: r.size as u64,
                    original: r.original,
                })
                .collect(),
            Err(e) => {
                tracing::error!(error = %e, "exact scoped dictionary index query failed");
                Vec::new()
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword)))]
    async fn exact_global(&self, headword: &str) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word, offset, size, original
               FROM dictionary_index_entry
               WHERE word = ?"#,
            headword,
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|r| Entry {
                    headword: r.word,
                    offset: r.offset as u64,
                    size: r.size as u64,
                    original: r.original,
                })
                .collect(),
            Err(e) => {
                tracing::error!(error = %e, "exact global dictionary index query failed");
                Vec::new()
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword, prefix = %prefix)))]
    async fn fuzzy_scoped(&self, headword: &str, prefix: &str, fp: &str) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word, offset, size, original
               FROM dictionary_index_entry
               WHERE fingerprint = ? AND word LIKE ? || '%' ESCAPE '\'"#,
            fp,
            prefix,
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows
                .into_iter()
                .filter(|r| levenshtein(headword, &r.word) <= 1)
                .map(|r| Entry {
                    headword: r.word,
                    offset: r.offset as u64,
                    size: r.size as u64,
                    original: r.original,
                })
                .collect(),
            Err(e) => {
                tracing::error!(error = %e, "fuzzy scoped dictionary index query failed");
                Vec::new()
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword, prefix = %prefix)))]
    async fn fuzzy_global(&self, headword: &str, prefix: &str) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word, offset, size, original
               FROM dictionary_index_entry
               WHERE word LIKE ? || '%' ESCAPE '\'"#,
            prefix,
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows
                .into_iter()
                .filter(|r| levenshtein(headword, &r.word) <= 1)
                .map(|r| Entry {
                    headword: r.word,
                    offset: r.offset as u64,
                    size: r.size as u64,
                    original: r.original,
                })
                .collect(),
            Err(e) => {
                tracing::error!(error = %e, "fuzzy global dictionary index query failed");
                Vec::new()
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword )))]
    fn query_exact(&self, headword: &str) -> Vec<Entry> {
        let headword = headword.to_string();

        RUNTIME.block_on(async {
            if let Some(fp) = self.fingerprint.map(|f| f.to_string()) {
                self.exact_scoped(&headword, &fp).await
            } else {
                self.exact_global(&headword).await
            }
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword )))]
    fn query_fuzzy(&self, headword: &str) -> Vec<Entry> {
        let prefix_len = headword
            .char_indices()
            .nth(3)
            .map(|(i, _)| i)
            .unwrap_or(headword.len());
        let prefix = escape_like_prefix(&headword[..prefix_len]);
        let headword = headword.to_string();

        RUNTIME.block_on(async {
            if let Some(fp) = self.fingerprint.map(|f| f.to_string()) {
                self.fuzzy_scoped(&headword, &prefix, &fp).await
            } else {
                self.fuzzy_global(&headword, &prefix).await
            }
        })
    }
}

impl IndexReader for DbIndexReader {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _metadata), fields(headword = %headword, fuzzy)))]
    fn load_and_find(&mut self, headword: &str, fuzzy: bool, _metadata: &Metadata) -> Vec<Entry> {
        self.find(headword, fuzzy)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword, fuzzy)))]
    fn find(&self, headword: &str, fuzzy: bool) -> Vec<Entry> {
        if fuzzy {
            self.query_fuzzy(headword)
        } else {
            self.query_exact(headword)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::runtime::RUNTIME;

    fn setup_db() -> Database {
        let db = Database::new(":memory:").expect("in-memory db");
        db.migrate().expect("migrations");
        db
    }

    fn insert_meta(pool: &SqlitePool, fp: &str) {
        RUNTIME.block_on(async {
            sqlx::query!(
                "INSERT OR IGNORE INTO dictionary_index_meta (fingerprint, dict_path, total_lines, indexed_lines, completed) VALUES (?, ?, 0, 0, 1)",
                fp,
                fp,
            )
            .execute(pool)
            .await
            .expect("insert meta");
        });
    }

    fn insert_entry(
        pool: &SqlitePool,
        fp: &str,
        word: &str,
        offset: i64,
        size: i64,
        original: Option<&str>,
    ) {
        insert_meta(pool, fp);
        RUNTIME.block_on(async {
            sqlx::query!(
                "INSERT INTO dictionary_index_entry (fingerprint, word, offset, size, original) VALUES (?, ?, ?, ?, ?)",
                fp,
                word,
                offset,
                size,
                original,
            )
            .execute(pool)
            .await
            .expect("insert entry");
        });
    }

    fn fp1() -> Fp {
        Fp::from_u64(1)
    }

    fn fp2() -> Fp {
        Fp::from_u64(2)
    }

    #[test]
    fn test_exact_lookup_with_fingerprint() {
        let db = setup_db();
        insert_entry(db.pool(), &fp1().to_string(), "hello", 0, 10, None);
        insert_entry(db.pool(), &fp2().to_string(), "world", 10, 5, None);

        let reader = DbIndexReader::new(&db, Some(fp1()));
        let results = reader.find("hello", false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].headword, "hello");
        assert_eq!(results[0].offset, 0);
        assert_eq!(results[0].size, 10);
    }

    #[test]
    fn test_exact_lookup_scoped_fingerprint_excludes_other() {
        let db = setup_db();
        insert_entry(db.pool(), &fp1().to_string(), "hello", 0, 10, None);
        insert_entry(db.pool(), &fp2().to_string(), "hello", 20, 8, None);

        let reader = DbIndexReader::new(&db, Some(fp1()));
        let results = reader.find("hello", false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].offset, 0);
    }

    #[test]
    fn test_exact_lookup_no_fingerprint_finds_all() {
        let db = setup_db();
        insert_entry(db.pool(), &fp1().to_string(), "hello", 0, 10, None);
        insert_entry(db.pool(), &fp2().to_string(), "hello", 20, 8, None);

        let reader = DbIndexReader::new(&db, None);
        let results = reader.find("hello", false);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_exact_lookup_no_match() {
        let db = setup_db();
        insert_entry(db.pool(), &fp1().to_string(), "hello", 0, 10, None);

        let reader = DbIndexReader::new(&db, Some(fp1()));
        let results = reader.find("world", false);
        assert!(results.is_empty());
    }

    #[test]
    fn test_fuzzy_lookup_with_fingerprint() {
        let db = setup_db();
        insert_entry(db.pool(), &fp1().to_string(), "hello", 0, 10, None);
        insert_entry(db.pool(), &fp1().to_string(), "helo", 10, 5, None);
        insert_entry(db.pool(), &fp1().to_string(), "world", 15, 5, None);

        let reader = DbIndexReader::new(&db, Some(fp1()));
        let results = reader.find("hello", true);
        assert_eq!(results.len(), 2);
        let words: Vec<&str> = results.iter().map(|e| e.headword.as_str()).collect();
        assert!(words.contains(&"hello"));
        assert!(words.contains(&"helo"));
    }

    #[test]
    fn test_fuzzy_lookup_no_fingerprint_cross_dict() {
        let db = setup_db();
        insert_entry(db.pool(), &fp1().to_string(), "hello", 0, 10, None);
        insert_entry(db.pool(), &fp2().to_string(), "helo", 10, 5, None);

        let reader = DbIndexReader::new(&db, None);
        let results = reader.find("hello", true);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_load_and_find_delegates_to_find() {
        let db = setup_db();
        insert_entry(db.pool(), &fp1().to_string(), "hello", 0, 10, None);

        let mut reader = DbIndexReader::new(&db, Some(fp1()));
        let metadata = Metadata {
            all_chars: true,
            case_sensitive: false,
        };
        let results = reader.load_and_find("hello", false, &metadata);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].headword, "hello");
    }

    #[test]
    fn test_original_field_preserved() {
        let db = setup_db();
        insert_entry(db.pool(), &fp1().to_string(), "hello", 0, 10, Some("Hello"));

        let reader = DbIndexReader::new(&db, Some(fp1()));
        let results = reader.find("hello", false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].original.as_deref(), Some("Hello"));
    }
}
