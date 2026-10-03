//! SQLite-backed dictionary index reader.
//!
//! Replaces the in-memory `.index` file reader with a database-backed implementation
//! that supports both single-dictionary and cross-dictionary word lookups.

use levenshtein::levenshtein;
use sqlx::SqlitePool;

use crate::db::Database;

use super::Metadata;
use super::indexing::{Entry, IndexReader};

/// Exclusive upper bound for `word >= prefix` in SQLite `TEXT` order.
fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    while let Some(c) = chars.pop() {
        if let Some(next) = char::from_u32(c as u32 + 1) {
            chars.push(next);
            return Some(chars.into_iter().collect());
        }
    }
    None
}

/// SQLite-backed implementation of [`IndexReader`].
///
/// When `dict_id` is `Some`, queries are scoped to that dictionary.
/// When `None`, queries search across all indexed dictionaries.
pub struct DbIndexReader {
    pool: SqlitePool,
    dict_id: Option<i64>,
}

impl DbIndexReader {
    /// Creates a new reader backed by `database`, optionally scoped to `dict_id`.
    pub fn new(database: &Database, dict_id: Option<i64>) -> Self {
        Self {
            pool: database.pool().clone(),
            dict_id,
        }
    }

    // TODO: exact_scoped, exact_global, fuzzy_scoped and fuzzy_global are four
    // copies of one query differing only by the optional dict_id predicate.
    // Collapse them into query_entries(headword, prefix, Option<i64>, fuzzy).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword)))]
    async fn exact_scoped(&self, headword: &str, id: i64) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word,
                      offset AS "offset!",
                      size AS "size!",
                      original
               FROM dictionary_index_entry
               WHERE dict_id = ? AND word = ?"#,
            id,
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
            r#"SELECT word,
                      offset AS "offset!",
                      size AS "size!",
                      original
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
    async fn fuzzy_scoped(
        &self,
        headword: &str,
        prefix: &str,
        prefix_end: &str,
        id: i64,
    ) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word,
                      offset AS "offset!",
                      size AS "size!",
                      original
               FROM dictionary_index_entry
               WHERE dict_id = ? AND word >= ? AND word < ?"#,
            id,
            prefix,
            prefix_end,
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
    async fn fuzzy_global(&self, headword: &str, prefix: &str, prefix_end: &str) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word,
                      offset AS "offset!",
                      size AS "size!",
                      original
               FROM dictionary_index_entry
               WHERE word >= ? AND word < ?"#,
            prefix,
            prefix_end,
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
    async fn query_exact(&self, headword: &str) -> Vec<Entry> {
        let headword = headword.to_string();

        if let Some(id) = self.dict_id {
            self.exact_scoped(&headword, id).await
        } else {
            self.exact_global(&headword).await
        }
    }

    /// Candidate selection is a half-open BINARY range over the `word` index,
    /// so it is case-sensitive: a `hello` prefix does not consider `Hello`.
    /// Case-insensitive dictionaries are unaffected because their words are
    /// lowercased at index time.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword )))]
    async fn query_fuzzy(&self, headword: &str) -> Vec<Entry> {
        let prefix_len = headword
            .char_indices()
            .nth(3)
            .map(|(i, _)| i)
            .unwrap_or(headword.len());
        let prefix = &headword[..prefix_len];
        let headword = headword.to_string();
        let upper = prefix_upper_bound(prefix);

        if let Some(upper) = upper {
            if let Some(id) = self.dict_id {
                return self.fuzzy_scoped(&headword, prefix, &upper, id).await;
            }
            return self.fuzzy_global(&headword, prefix, &upper).await;
        }

        if let Some(id) = self.dict_id {
            self.fuzzy_scoped_unbounded(&headword, prefix, id).await
        } else {
            self.fuzzy_global_unbounded(&headword, prefix).await
        }
    }

    async fn fuzzy_scoped_unbounded(&self, headword: &str, prefix: &str, id: i64) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word,
                      offset AS "offset!",
                      size AS "size!",
                      original
               FROM dictionary_index_entry
               WHERE dict_id = ? AND word >= ?"#,
            id,
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
                tracing::error!(error = %e, "fuzzy scoped unbounded dictionary index query failed");
                Vec::new()
            }
        }
    }

    async fn fuzzy_global_unbounded(&self, headword: &str, prefix: &str) -> Vec<Entry> {
        match sqlx::query!(
            r#"SELECT word,
                      offset AS "offset!",
                      size AS "size!",
                      original
               FROM dictionary_index_entry
               WHERE word >= ?"#,
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
                tracing::error!(error = %e, "fuzzy global unbounded dictionary index query failed");
                Vec::new()
            }
        }
    }
}

#[async_trait::async_trait]
impl IndexReader for DbIndexReader {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _metadata), fields(headword = %headword, fuzzy)))]
    async fn load_and_find(
        &mut self,
        headword: &str,
        fuzzy: bool,
        _metadata: &Metadata,
    ) -> Vec<Entry> {
        self.find(headword, fuzzy).await
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(headword = %headword, fuzzy)))]
    async fn find(&self, headword: &str, fuzzy: bool) -> Vec<Entry> {
        if fuzzy {
            self.query_fuzzy(headword).await
        } else {
            self.query_exact(headword).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> Database {
        let mut db = Database::new(":memory:").await.expect("in-memory db");
        db.init_for_test(0).await.expect("migrations");
        db
    }

    async fn insert_meta(pool: &SqlitePool, dict_id: i64, fp: &str) {
        sqlx::query!(
                "INSERT OR IGNORE INTO dictionary_index_meta (dict_id, fingerprint, dict_path, total_lines, indexed_lines, completed) VALUES (?, ?, ?, 0, 0, 1)",
                dict_id,
                fp,
                fp,
            )
            .execute(pool)
            .await
            .expect("insert meta");
    }

    async fn insert_entry(
        pool: &SqlitePool,
        dict_id: i64,
        fp: &str,
        word: &str,
        offset: i64,
        size: i64,
        original: Option<&str>,
    ) {
        insert_meta(pool, dict_id, fp).await;
        sqlx::query!(
                "INSERT INTO dictionary_index_entry (dict_id, word, offset, size, original) VALUES (?, ?, ?, ?, ?)",
                dict_id,
                word,
                offset,
                size,
                original,
            )
            .execute(pool)
            .await
            .expect("insert entry");
    }

    const DICT_ID_1: i64 = 1;
    const DICT_ID_2: i64 = 2;

    #[test]
    fn prefix_upper_bound_increments_last_scalar() {
        assert_eq!(prefix_upper_bound("aba").as_deref(), Some("abb"));
        assert_eq!(prefix_upper_bound("ab%").as_deref(), Some("ab&"));
        assert_eq!(prefix_upper_bound("a").as_deref(), Some("b"));
    }

    #[test]
    fn prefix_upper_bound_skips_non_incrementable_suffix() {
        let max = '\u{10FFFF}';
        let prefix = format!("x{max}");
        assert_eq!(prefix_upper_bound(&prefix).as_deref(), Some("y"));
    }

    #[test]
    fn prefix_upper_bound_returns_none_when_no_successor() {
        assert_eq!(prefix_upper_bound(""), None);
        assert_eq!(prefix_upper_bound("\u{10FFFF}"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_fuzzy_prefix_range_respects_literal_wildcard_chars() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "ab%word", 0, 10, None).await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "abxword", 10, 10, None).await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "abyword", 20, 10, None).await;

        let reader = DbIndexReader::new(&db, Some(DICT_ID_1));
        let results = reader.find("ab%word", true).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].headword, "ab%word");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_fuzzy_prefix_candidates_are_case_sensitive() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, None).await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "Hello", 10, 10, None).await;

        let reader = DbIndexReader::new(&db, Some(DICT_ID_1));

        let lower = reader.find("hello", true).await;
        assert_eq!(lower.len(), 1);
        assert_eq!(lower[0].headword, "hello");

        let upper = reader.find("Hello", true).await;
        assert_eq!(upper.len(), 1);
        assert_eq!(upper[0].headword, "Hello");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_exact_lookup_with_dict_id() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, None).await;
        insert_entry(db.pool(), DICT_ID_2, "fp2", "world", 10, 5, None).await;

        let reader = DbIndexReader::new(&db, Some(DICT_ID_1));
        let results = reader.find("hello", false).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].headword, "hello");
        assert_eq!(results[0].offset, 0);
        assert_eq!(results[0].size, 10);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_exact_lookup_scoped_dict_id_excludes_other() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, None).await;
        insert_entry(db.pool(), DICT_ID_2, "fp2", "hello", 20, 8, None).await;

        let reader = DbIndexReader::new(&db, Some(DICT_ID_1));
        let results = reader.find("hello", false).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].offset, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_exact_lookup_no_dict_id_finds_all() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, None).await;
        insert_entry(db.pool(), DICT_ID_2, "fp2", "hello", 20, 8, None).await;

        let reader = DbIndexReader::new(&db, None);
        let results = reader.find("hello", false).await;
        assert_eq!(results.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_exact_lookup_no_match() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, None).await;

        let reader = DbIndexReader::new(&db, Some(DICT_ID_1));
        let results = reader.find("world", false).await;
        assert!(results.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_fuzzy_lookup_with_dict_id() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, None).await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "helo", 10, 5, None).await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "world", 15, 5, None).await;

        let reader = DbIndexReader::new(&db, Some(DICT_ID_1));
        let results = reader.find("hello", true).await;
        assert_eq!(results.len(), 2);
        let words: Vec<&str> = results.iter().map(|e| e.headword.as_str()).collect();
        assert!(words.contains(&"hello"));
        assert!(words.contains(&"helo"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_fuzzy_lookup_no_dict_id_cross_dict() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, None).await;
        insert_entry(db.pool(), DICT_ID_2, "fp2", "helo", 10, 5, None).await;

        let reader = DbIndexReader::new(&db, None);
        let results = reader.find("hello", true).await;
        assert_eq!(results.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_and_find_delegates_to_find() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, None).await;

        let mut reader = DbIndexReader::new(&db, Some(DICT_ID_1));
        let metadata = Metadata {
            all_chars: true,
            case_sensitive: false,
        };
        let results = reader.load_and_find("hello", false, &metadata).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].headword, "hello");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_original_field_preserved() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "hello", 0, 10, Some("Hello")).await;

        let reader = DbIndexReader::new(&db, Some(DICT_ID_1));
        let results = reader.find("hello", false).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].original.as_deref(), Some("Hello"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_multiple_definitions_same_word_all_returned() {
        let db = setup_db().await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "pain", 100, 20, Some("Pain")).await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "pain", 200, 30, Some("PAIN")).await;
        insert_entry(db.pool(), DICT_ID_1, "fp1", "pain", 300, 40, None).await;

        let reader = DbIndexReader::new(&db, Some(DICT_ID_1));
        let results = reader.find("pain", false).await;
        assert_eq!(results.len(), 3);
        let offsets: Vec<u64> = results.iter().map(|e| e.offset).collect();
        assert!(offsets.contains(&100));
        assert!(offsets.contains(&200));
        assert!(offsets.contains(&300));
    }
}
