//! API response types for the monolingual dictionary metadata endpoint.
//!
//! The `GET https://www.reader-dict.com/api/v1/dictionaries` endpoint returns
//! a unified bilingual + monolingual registry. This module only models and
//! exposes the **monolingual** subset (entries where source language equals
//! target language, e.g. `en → en`). Bilingual pairs are ignored.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Top-level response from `GET https://www.reader-dict.com/api/v1/dictionaries`.
///
/// The API returns a nested map of source language → target language → entry.
/// Both monolingual (src == tgt) and bilingual (src != tgt) entries are present,
/// but only the monolingual subset is used by this module.
pub type DictionariesResponse = HashMap<String, HashMap<String, DictionaryEntry>>;

/// A single dictionary entry returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DictionaryEntry {
    /// Comma-separated list of available download formats
    /// (e.g. `"df,dic,dictorg,kobo,mobi,stardict"`).
    pub formats: String,

    /// ISO 8601 date of the last release (e.g. `"2026-04-01"`).
    pub updated: String,

    /// Number of headword entries in the dictionary.
    pub words: u64,
}

/// Returns the download URL for the DICT.org format archive (includes etymologies).
///
/// Pattern: `https://www.reader-dict.com/file/{lang}/dictorg-{lang}-{lang}.zip`
pub(super) fn download_url(lang: &str) -> String {
    format!(
        "https://www.reader-dict.com/file/{lang}/dictorg-{lang}-{lang}.zip",
        lang = lang
    )
}

/// Returns the download URL for the DICT.org format archive **without** etymologies.
///
/// Pattern: `https://www.reader-dict.com/file/{lang}/dictorg-{lang}-{lang}-noetym.zip`
pub(super) fn download_url_no_etym(lang: &str) -> String {
    format!(
        "https://www.reader-dict.com/file/{lang}/dictorg-{lang}-{lang}-noetym.zip",
        lang = lang
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry() -> DictionaryEntry {
        DictionaryEntry {
            formats: "df,dic,dictorg,kobo,mobi,stardict".to_string(),
            updated: "2026-04-01".to_string(),
            words: 1_381_375,
        }
    }

    #[test]
    fn test_download_url_english() {
        assert_eq!(
            download_url("en"),
            "https://www.reader-dict.com/file/en/dictorg-en-en.zip"
        );
    }

    #[test]
    fn test_download_url_no_etym_english() {
        assert_eq!(
            download_url_no_etym("en"),
            "https://www.reader-dict.com/file/en/dictorg-en-en-noetym.zip"
        );
    }

    #[test]
    fn test_download_url_french() {
        assert_eq!(
            download_url("fr"),
            "https://www.reader-dict.com/file/fr/dictorg-fr-fr.zip"
        );
    }

    #[test]
    fn test_deserialize_response() {
        let json = r#"{
            "en": {
                "en": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-04-01", "words": 1381375 },
                "fr": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-04-01", "words": 50000 }
            },
            "fr": {
                "fr": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-03-01", "words": 2050655 }
            }
        }"#;

        let resp: DictionariesResponse = serde_json::from_str(json).unwrap();

        let en_entry = resp.get("en").and_then(|m| m.get("en")).unwrap();
        assert_eq!(en_entry.words, 1_381_375);
        assert_eq!(en_entry.updated, "2026-04-01");

        let fr_entry = resp.get("fr").and_then(|m| m.get("fr")).unwrap();
        assert_eq!(fr_entry.words, 2_050_655);

        assert_eq!(*en_entry, make_entry());
    }

    #[test]
    fn test_monolingual_filter() {
        let json = r#"{
            "en": {
                "en": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-04-01", "words": 1381375 },
                "fr": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-04-01", "words": 50000 }
            },
            "af": {
                "en": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-04-01", "words": 8934 }
            }
        }"#;

        let resp: DictionariesResponse = serde_json::from_str(json).unwrap();

        let monolingual: Vec<(&str, &DictionaryEntry)> = resp
            .iter()
            .filter_map(|(lang, targets)| targets.get(lang.as_str()).map(|e| (lang.as_str(), e)))
            .collect();

        assert_eq!(monolingual.len(), 1);
        assert_eq!(monolingual[0].0, "en");
    }
}
