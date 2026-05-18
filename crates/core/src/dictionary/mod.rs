//! A dict format (`*.dict`) reader crate.
//!
//! This crate can read dictionaries in the dict format, as used by dictd. It supports both
//! uncompressed and compressed dictionaries.
//!
//! It also provides support for downloading dictionaries from the monolingual project.

mod dictreader;
mod errors;
#[cfg(feature = "bench")]
pub mod indexing;
#[cfg(not(feature = "bench"))]
mod indexing;

pub(crate) mod db_index;
mod monolingual;

pub(crate) use monolingual::MonolingualDictionaryService;

use std::path::Path;

use self::dictreader::DictReader;
use self::indexing::IndexReader;
use crate::db::Database;
use crate::helpers::Fp;

/// A dictionary wrapper.
///
/// A dictionary is made up of a `*.dict` or `*.dict.dz` file with the actual content and a
/// `*.index` file with a list of all headwords and with positions in the dict file + length
/// information. It provides a convenience function to look up headwords directly, without caring
/// about the details of the index and the underlying dict format.
pub struct Dictionary {
    content: Box<dyn DictReader>,
    index: Box<dyn IndexReader>,
    metadata: Metadata,
}

/// The special metadata entries that we care about.
///
/// These entries should appear close to the beginning of the index file.
pub struct Metadata {
    pub all_chars: bool,
    pub case_sensitive: bool,
}

impl Dictionary {
    /// Look up a word in a dictionary.
    ///
    /// Words are looked up in the index and then retrieved from the dict file. If no word was
    /// found, the returned vector is empty. Errors result from the parsing of the underlying files.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(word = %word, fuzzy)))]
    pub fn lookup(
        &mut self,
        word: &str,
        fuzzy: bool,
    ) -> Result<Vec<[String; 2]>, errors::DictError> {
        let mut query = word.to_string();
        if !self.metadata.case_sensitive {
            query = query.to_lowercase();
        }
        if !self.metadata.all_chars {
            query = query
                .chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect();
        }
        let entries = self.index.load_and_find(&query, fuzzy, &self.metadata);
        let mut results = Vec::new();
        for entry in entries.into_iter() {
            results.push([
                entry.original.unwrap_or(entry.headword),
                self.content.fetch_definition(entry.offset, entry.size)?,
            ]);
        }
        Ok(results)
    }

    /// Retreive metadata from the dictionaries.
    ///
    /// The metadata headwords start with `00-database-` or `00database`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(name = %name)))]
    pub fn metadata(&mut self, name: &str) -> Result<String, errors::DictError> {
        let mut query = format!("00-database-{}", name);
        if !self.metadata.all_chars {
            query = query.replace(|c: char| !c.is_alphanumeric(), "");
        }
        let entries = self.index.find(&query, false);
        let entry = entries
            .get(0)
            .ok_or_else(|| errors::DictError::WordNotFound(name.into()))?;
        self.content
            .fetch_definition(entry.offset, entry.size)
            .map(|def| {
                let start = def
                    .find('\n')
                    .filter(|pos| *pos < def.len() - 1)
                    .unwrap_or(0);
                def[start..].trim().to_string()
            })
    }

    /// Get the short name.
    ///
    /// This returns the short name of a dictionary. This corresponds to the
    /// value passed to the `-s` option of `dictfmt`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn short_name(&mut self) -> Result<String, errors::DictError> {
        self.metadata("short")
    }

    /// Get the URL.
    ///
    /// This returns the URL of a dictionary. This corresponds to the
    /// value passed to the `-u` option of `dictfmt`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn url(&mut self) -> Result<String, errors::DictError> {
        self.metadata("url")
    }
}

/// Load dictionary using a database-backed index reader.
///
/// The content file is read from disk; index lookups are served from the
/// database. Works even when indexing is still in progress — partial results
/// are returned for words already indexed.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(database), fields(fingerprint = %fingerprint)))]
pub fn load_dictionary_from_db<P: AsRef<Path> + std::fmt::Debug>(
    content_path: P,
    database: &Database,
    fingerprint: Fp,
) -> Result<Dictionary, errors::DictError> {
    let content = dictreader::load_dict(content_path)?;
    let index = Box::new(db_index::DbIndexReader::new(database, Some(fingerprint)));
    Ok(load_dictionary(content, index))
}

/// Load dictionary from given `DictReader` and `Index`.
///
/// A dictionary is made of an index and a dictionary (data). Both are required for look up. This
/// function allows abstraction from the underlying source by only requiring a
/// `DictReader` and an [`IndexReader`].
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub fn load_dictionary(content: Box<dyn DictReader>, index: Box<dyn IndexReader>) -> Dictionary {
    let all_chars = !index.find("00-database-allchars", false).is_empty();
    let word = if all_chars {
        "00-database-case-sensitive"
    } else {
        "00databasecasesensitive"
    };
    let case_sensitive = !index.find(word, false).is_empty();
    Dictionary {
        content,
        index,
        metadata: Metadata {
            all_chars,
            case_sensitive,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::runtime::RUNTIME;

    const PATH_CASE_SENSITIVE_DICT: &str = "src/dictionary/testdata/case_sensitive_dict.dict";
    const PATH_CASE_INSENSITIVE_DICT: &str = "src/dictionary/testdata/case_insensitive_dict.dict";
    type TestEntry = (&'static str, i64, i64);

    const CASE_INSENSITIVE_ENTRIES: &[TestEntry] = &[
        ("00-database-allchars", 1, 1),
        ("bar", 443, 30),
        ("foo", 428, 15),
        ("straße", 516, 44),
    ];

    const CASE_SENSITIVE_ENTRIES: &[TestEntry] = &[
        ("00-database-allchars", 1, 1),
        ("00-database-case-sensitive", 2, 1),
        ("Bar", 459, 30),
        ("foo", 444, 15),
        ("straße", 532, 44),
    ];

    fn load_test_dictionary(
        content_path: &str,
        entries: &[TestEntry],
    ) -> Result<Dictionary, errors::DictError> {
        let db = Database::new(":memory:").expect("in-memory db");
        db.migrate().expect("migrations");

        let fp = Fp::from_u64(1);
        let fp_str = fp.to_string();

        RUNTIME.block_on(async {
            let fingerprint = fp_str.clone();
            let fingerprint_ref = fingerprint.as_str();
            sqlx::query!(
                r#"INSERT INTO dictionary_index_meta (fingerprint, dict_path, total_lines, indexed_lines, completed)
                   VALUES (?, ?, ?, 0, 0)"#,
                fingerprint_ref,
                content_path,
                0_i64,
            )
            .execute(db.pool())
            .await
            .expect("insert meta");

            for (word, offset, size) in entries {
                let fingerprint_ref = fingerprint.as_str();
                sqlx::query!(
                    r#"INSERT OR IGNORE INTO dictionary_index_entry (fingerprint, word, offset, size, original)
                       VALUES (?, ?, ?, ?, ?)"#,
                    fingerprint_ref,
                    word,
                    offset,
                    size,
                    Option::<&str>::None,
                )
                .execute(db.pool())
                .await
                .expect("insert entry");
            }
        });

        load_dictionary_from_db(content_path, &db, fp)
    }

    fn assert_dict_word_exists(
        mut dict: Dictionary,
        headword: &str,
        definition: &str,
    ) -> Dictionary {
        let r = dict.lookup(headword, false);
        assert!(r.is_ok());
        let search = r.unwrap();
        assert_eq!(search.len(), 1);
        assert!(search[0][1].contains(definition));

        dict
    }

    #[test]
    fn test_load_dictionary_from_db() {
        let r = load_test_dictionary(PATH_CASE_INSENSITIVE_DICT, CASE_INSENSITIVE_ENTRIES);
        assert!(r.is_ok());
    }

    #[test]
    fn test_dictionary_lookup_case_insensitive() {
        let r = load_test_dictionary(PATH_CASE_INSENSITIVE_DICT, CASE_INSENSITIVE_ENTRIES);
        let mut dict = r.unwrap();

        dict = assert_dict_word_exists(dict, "bar", "test for case-sensitivity");
        dict = assert_dict_word_exists(dict, "Bar", "test for case-sensitivity");
        assert_dict_word_exists(dict, "straße", "test for non-latin case-sensitivity");
    }

    #[test]
    fn test_dictionary_lookup_case_insensitive_fuzzy() {
        let r = load_test_dictionary(PATH_CASE_INSENSITIVE_DICT, CASE_INSENSITIVE_ENTRIES);
        let mut dict = r.unwrap();

        let r = dict.lookup("ba", true);
        assert!(r.is_ok());
        let search = r.unwrap();
        assert_eq!(search.len(), 1);
        assert_eq!(search[0][0], "bar");
        assert!(search[0][1].contains("test for case-sensitivity"));
    }

    #[test]
    fn test_dictionary_lookup_case_sensitive() {
        let r = load_test_dictionary(PATH_CASE_SENSITIVE_DICT, CASE_SENSITIVE_ENTRIES);
        let mut dict = r.unwrap();

        dict = assert_dict_word_exists(dict, "Bar", "test for case-sensitivity");
        dict = assert_dict_word_exists(dict, "straße", "test for non-latin case-sensitivity");

        let r = dict.lookup("bar", false);
        assert!(r.unwrap().is_empty());

        let r = dict.lookup("strasse", false);
        assert!(r.unwrap().is_empty());
    }

    #[test]
    fn test_dictionary_lookup_case_sensitive_fuzzy() {
        let r = load_test_dictionary(PATH_CASE_SENSITIVE_DICT, CASE_SENSITIVE_ENTRIES);
        let mut dict = r.unwrap();

        let r = dict.lookup("Ba", true);
        assert!(r.is_ok());
        let search = r.unwrap();
        assert_eq!(search.len(), 1);
        assert_eq!(search[0][0], "Bar");
        assert!(search[0][1].contains("test for case-sensitivity"));
    }
}
