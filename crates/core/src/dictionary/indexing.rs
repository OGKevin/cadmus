//! Shared types for dictionary index readers.

use super::Metadata;

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

/// Applies case and character normalization to a headword.
///
/// Used at index time to normalize stored words and at query time to normalize
/// the lookup term so both sides use identical transformations.
pub(crate) fn apply_transform(
    headword: &str,
    needs_char_filter: bool,
    needs_lowercase: bool,
) -> String {
    let filtered: String = if needs_char_filter {
        headword
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect()
    } else {
        headword.to_owned()
    };

    if needs_lowercase {
        filtered.to_lowercase()
    } else {
        filtered
    }
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

    if is_sorted(&result) {
        return result;
    }

    result.sort_by_cached_key(|e| e.headword.clone());
    result
}

fn is_sorted(entries: &[Entry]) -> bool {
    entries.windows(2).all(|w| w[0].headword <= w[1].headword)
}

/// Normalize entries based on dictionary metadata.
///
/// If no normalization is needed and the entries are already sorted, returns
/// the original entries unchanged. Otherwise transforms headwords (lowercasing
/// and/or stripping non-alphanumeric characters) and sorts by headword.
#[cfg(feature = "bench")]
pub fn normalize(entries: &[Entry], metadata: &Metadata) -> Vec<Entry> {
    normalize_internal(entries, metadata)
}

#[cfg(not(feature = "bench"))]
pub(crate) fn normalize(entries: &[Entry], metadata: &Metadata) -> Vec<Entry> {
    normalize_internal(entries, metadata)
}
