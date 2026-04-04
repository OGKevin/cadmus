//! Parse and decode `*.index` files.
//!
//! Each dictionary file (`*.dict.dz)`) is accompanied by a `*.index` file containing a list of
//! words, together with its (byte) position in the dict file and its (byte) length. This module
//! provides functions to parse this index file.
//!
//! The position and the length of a definition is given in a semi-base64 encoding. It uses all
//! Latin letters (upper and lower case), all digits and additionally, `+` and `/`:
//!
//! `ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/`
//!
//! The calculation works as follows: `sum += x * 64^i`
//!
//! - `i` is the position within the string to calculate the number from and counts from right to
//!   left, starting at 0.
//! - `x` is the index within the array given above, i.e. `'a' == 26`.
//!
//! The sum makes up the index.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use levenshtein::levenshtein;

use super::errors::DictError;
use super::errors::DictError::*;
use super::Metadata;

/// The index is partially loaded if `state` isn't `None`.
pub struct Index<R: BufRead> {
    pub entries: Vec<Entry>,
    pub state: Option<R>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub headword: String,
    pub offset: u64,
    pub size: u64,
    pub original: Option<String>,
}

pub trait IndexReader {
    fn load_and_find(&mut self, headword: &str, fuzzy: bool, metadata: &Metadata) -> Vec<Entry>;
    fn find(&self, headword: &str, fuzzy: bool) -> Vec<Entry>;
}

fn normalize_internal(entries: &[Entry], metadata: &Metadata) -> Vec<Entry> {
    let needs_char_filter = !metadata.all_chars;
    let needs_lowercase = !metadata.case_sensitive;

    if !needs_char_filter && !needs_lowercase && is_sorted(entries) {
        return entries.to_vec();
    }

    let mut result: Vec<Entry> = entries
        .iter()
        .map(|entry| {
            let transformed = apply_transform(&entry.headword, needs_char_filter, needs_lowercase);

            let original = if transformed != entry.headword {
                Some(entry.headword.clone())
            } else {
                None
            };

            Entry {
                headword: transformed,
                offset: entry.offset,
                size: entry.size,
                original,
            }
        })
        .collect();

    {
        #[cfg(feature = "otel")]
        let _span = tracing::info_span!("checking if already sorted").entered();
        if is_sorted(&result) {
            return result;
        }
    }

    #[cfg(feature = "otel")]
    tracing::info_span!("sorting").in_scope(|| {
        result.sort_by_cached_key(|e| e.headword.clone());
    });

    #[cfg(not(feature = "otel"))]
    result.sort_by_cached_key(|e| e.headword.clone());

    result
}

#[cfg(feature = "bench")]
pub fn normalize(entries: &[Entry], metadata: &Metadata) -> Vec<Entry> {
    normalize_internal(entries, metadata)
}

/// Normalize the entries based on the metadata. If no normalization is needed and the entries are
/// already sorted, the original entries are returned. Otherwise, a new vector of entries is
/// returned, with the headwords transformed as needed and sorted by headword.
#[cfg_attr(feature = "otel", tracing::instrument(skip_all, fields(entry_count = entries.len())))]
#[cfg(not(feature = "bench"))]
fn normalize(entries: &[Entry], metadata: &Metadata) -> Vec<Entry> {
    normalize_internal(entries, metadata)
}

fn is_sorted(entries: &[Entry]) -> bool {
    entries.windows(2).all(|w| w[0].headword <= w[1].headword)
}

fn apply_transform(headword: &str, needs_char_filter: bool, needs_lowercase: bool) -> String {
    let filtered: String = if needs_char_filter {
        headword
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect()
    } else {
        headword.to_owned()
    };

    if needs_lowercase {
        return filtered.to_lowercase();
    }

    filtered
}

impl<R: BufRead> IndexReader for Index<R> {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, metadata), fields(headword = %headword, fuzzy)))]
    fn load_and_find(&mut self, headword: &str, fuzzy: bool, metadata: &Metadata) -> Vec<Entry> {
        if let Some(br) = self.state.take() {
            if let Ok(mut index) = parse_index(br, false) {
                self.entries.append(&mut index.entries);
                self.entries = normalize(&self.entries, metadata);
            }
        }
        self.find(headword, fuzzy)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(headword = %headword, fuzzy, entry_count = self.entries.len())))]
    fn find(&self, headword: &str, fuzzy: bool) -> Vec<Entry> {
        if fuzzy {
            self.entries
                .iter()
                .filter(|entry| levenshtein(headword, &entry.headword) <= 1)
                .cloned()
                .collect()
        } else {
            if let Ok(mut i) = self
                .entries
                .binary_search_by_key(&headword, |entry| &entry.headword)
            {
                let mut results = vec![self.entries[i].clone()];
                let j = i;
                while i > 0 {
                    i -= 1;
                    if self.entries[i].headword != headword {
                        break;
                    }
                    results.insert(0, self.entries[i].clone());
                }
                i = j;
                while i < self.entries.len() - 1 {
                    i += 1;
                    if self.entries[i].headword != headword {
                        break;
                    }
                    results.push(self.entries[i].clone());
                }
                results
            } else {
                Vec::new()
            }
        }
    }
}

/// Get the assigned number for a character
/// If the character was unknown, an empty Err(()) is returned.
#[inline]
fn get_base(input: char) -> Option<u64> {
    match input {
        'A'..='Z' => Some((input as u64) - 65), // 'A' should become 0
        'a'..='z' => Some((input as u64) - 71), // 'a' should become 26, ...
        '0'..='9' => Some((input as u64) + 4),  // 0 should become 52
        '+' => Some(62),
        '/' => Some(63),
        _ => None,
    }
}

/// Decode a number from a given String.
///
/// This function decodes a number from the format described in the module documentation. If
/// unknown characters/bytes are encountered, a `DictError` is returned.
pub fn decode_number(word: &str) -> Result<u64, DictError> {
    let mut index = 0u64;
    for (i, character) in word.chars().rev().enumerate() {
        index += match get_base(character) {
            Some(x) => x * 64u64.pow(i as u32),
            None => return Err(InvalidCharacter(character, None, Some(i))),
        };
    }
    Ok(index)
}

/// Parse a single line from the index file.
fn parse_line(line: &str, line_number: usize) -> Result<(&str, u64, u64, Option<&str>), DictError> {
    // First column: headword.
    let mut split = line.split('\t');
    let headword = split.next().ok_or(MissingColumnInIndex(line_number))?;

    // Second column: offset into file.
    let offset = split.next().ok_or(MissingColumnInIndex(line_number))?;
    let offset = decode_number(offset)?;

    // Third column: entry size.
    let size = split.next().ok_or(MissingColumnInIndex(line_number))?;
    let size = decode_number(size)?;

    // Fourth column: optional original headword.
    let original = split.next();

    Ok((headword, offset, size, original))
}

/// Parse the index for a dictionary from a given BufRead compatible object.
/// When `lazy` is `true`, the loop stops once all the metadata entries are parsed.
#[cfg_attr(feature = "otel", tracing::instrument(skip_all))]
pub fn parse_index<B: BufRead>(mut br: B, lazy: bool) -> Result<Index<B>, DictError> {
    let mut info = false;
    let mut entries = Vec::new();
    let mut line_number = 0;
    let mut line = String::new();

    while let Ok(nb) = br.read_line(&mut line) {
        if nb == 0 {
            break;
        }
        let (headword, offset, size, original) = parse_line(line.trim_end(), line_number)?;

        entries.push(Entry {
            headword: headword.to_string(),
            offset,
            size,
            original: original.map(String::from),
        });

        if lazy {
            if !info && (headword.starts_with("00-database-") || headword.starts_with("00database"))
            {
                info = true;
            } else if info
                && !headword.starts_with("00-database-")
                && !headword.starts_with("00database")
            {
                break;
            }
        }
        line_number += 1;
        line.clear();
    }

    let state = if lazy { Some(br) } else { None };

    Ok(Index { entries, state })
}

/// Parse the index for a dictionary from a given path.
#[cfg_attr(feature = "otel", tracing::instrument(skip_all))]
pub fn parse_index_from_file<P: AsRef<Path>>(
    path: P,
    lazy: bool,
) -> Result<Index<BufReader<File>>, DictError> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    parse_index(reader, lazy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Empty;

    const PATH_CASE_SENSITIVE_INDEX: &str = "src/dictionary/testdata/case_sensitive_dict.index";
    const PATH_CASE_INSENSITIVE_INDEX: &str = "src/dictionary/testdata/case_insensitive_dict.index";

    #[test]
    fn test_index_find() {
        let words = vec![
            Entry {
                headword: String::from("bar"),
                offset: 0,
                size: 8,
                original: None,
            },
            Entry {
                headword: String::from("baz"),
                offset: 8,
                size: 4,
                original: None,
            },
            Entry {
                headword: String::from("foo"),
                offset: 12,
                size: 4,
                original: None,
            },
        ];

        let index: Index<Empty> = Index {
            entries: words,
            state: None,
        };

        let r = index.find("apples", false);
        assert!(r.is_empty());

        let r = index.find("baz", false);
        assert!(!r.is_empty());
        assert_eq!(r.len(), 1);
        assert_eq!(r.first().unwrap().headword, "baz");

        let r = index.find("bas", true);
        assert!(!r.is_empty());
        assert_eq!(r.len(), 2);
        assert_eq!(r.first().unwrap().headword, "bar");
    }

    #[test]
    // Make sure that a lazy load does not inadvertently skip a word when it returns to BufRead
    fn test_index_load_and_find() {
        let r = parse_index_from_file(PATH_CASE_INSENSITIVE_INDEX, true);
        assert!(r.is_ok());

        let mut index = r.unwrap();
        assert_eq!(index.entries[0].headword, "00-database-allchars");
        assert_eq!(index.entries.last().unwrap().headword, "bar");

        let r = index.load_and_find(
            "bar",
            false,
            &Metadata {
                all_chars: true,
                case_sensitive: false,
            },
        );
        assert!(!r.is_empty());

        let r = index.load_and_find(
            "foo",
            false,
            &Metadata {
                all_chars: true,
                case_sensitive: false,
            },
        );
        assert!(!r.is_empty());
    }

    #[test]
    fn test_parse_index_from_file() {
        let r = parse_index_from_file(PATH_CASE_INSENSITIVE_INDEX, false);
        assert!(r.is_ok());

        let index = r.unwrap();
        assert_eq!(index.entries[0].headword, "00-database-allchars");
        assert_eq!(index.entries.last().unwrap().headword, "あいおい");
    }

    #[test]
    fn test_parse_index_from_file_lazy() {
        let r = parse_index_from_file(PATH_CASE_INSENSITIVE_INDEX, true);
        assert!(r.is_ok());

        let index = r.unwrap();
        assert_eq!(index.entries[0].headword, "00-database-allchars");
        assert_eq!(index.entries.last().unwrap().headword, "bar");
    }

    #[test]
    fn test_parse_index_from_file_handles_case_insensitivity() {
        let r = parse_index_from_file(PATH_CASE_INSENSITIVE_INDEX, false);
        assert!(r.is_ok());

        let index = r.unwrap();

        let r = index.find("bar", false);
        assert!(!r.is_empty());
        assert_eq!(r.first().unwrap().headword, "bar");
    }

    #[test]
    fn test_parse_index_from_file_handles_case_sensitivity() {
        let r = parse_index_from_file(PATH_CASE_SENSITIVE_INDEX, false);
        assert!(r.is_ok());

        let index = r.unwrap();

        let r = index.find("Bar", false);
        assert!(!r.is_empty());
        assert_eq!(r.first().unwrap().headword, "Bar");
    }

    fn make_entry(headword: &str) -> Entry {
        Entry {
            headword: headword.to_string(),
            offset: 0,
            size: 0,
            original: None,
        }
    }

    #[test]
    fn test_is_sorted_empty() {
        assert!(is_sorted(&[]));
    }

    #[test]
    fn test_is_sorted_single() {
        assert!(is_sorted(&[make_entry("a")]));
    }

    #[test]
    fn test_is_sorted_sorted() {
        assert!(is_sorted(&[
            make_entry("apple"),
            make_entry("banana"),
            make_entry("cherry")
        ]));
    }

    #[test]
    fn test_is_sorted_unsorted() {
        assert!(!is_sorted(&[make_entry("banana"), make_entry("apple")]));
    }

    #[test]
    fn test_apply_transform_identity() {
        assert_eq!(apply_transform("hello world", false, false), "hello world");
    }

    #[test]
    fn test_apply_transform_char_filter() {
        assert_eq!(
            apply_transform("he!llo, world.", true, false),
            "hello world"
        );
    }

    #[test]
    fn test_apply_transform_lowercase() {
        assert_eq!(apply_transform("Hello World", false, true), "hello world");
    }

    #[test]
    fn test_apply_transform_char_filter_and_lowercase() {
        assert_eq!(apply_transform("Hello, World!", true, true), "hello world");
    }

    #[test]
    fn test_normalize_no_transform_already_sorted() {
        let entries = vec![
            make_entry("apple"),
            make_entry("banana"),
            make_entry("cherry"),
        ];
        let metadata = Metadata {
            all_chars: true,
            case_sensitive: true,
        };
        let result = normalize(&entries, &metadata);
        assert_eq!(
            result.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            vec!["apple", "banana", "cherry"]
        );
    }

    #[test]
    fn test_normalize_transform_already_sorted() {
        let entries = vec![
            make_entry("apple"),
            make_entry("banana"),
            make_entry("cherry"),
        ];
        let metadata = Metadata {
            all_chars: false,
            case_sensitive: false,
        };
        let result = normalize(&entries, &metadata);
        assert_eq!(
            result.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            vec!["apple", "banana", "cherry"]
        );
    }

    #[test]
    fn test_normalize_transform_needs_sort() {
        let entries = vec![
            make_entry("Cherry"),
            make_entry("Apple!"),
            make_entry("banana"),
        ];
        let metadata = Metadata {
            all_chars: false,
            case_sensitive: false,
        };
        let result = normalize(&entries, &metadata);
        assert_eq!(
            result.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            vec!["apple", "banana", "cherry"]
        );
    }
}
