//! API response types for the monolingual dictionary metadata endpoint.
//!
//! The `GET https://www.reader-dict.com/api/v1/dictionaries` endpoint returns
//! a unified bilingual + monolingual registry. This module only models and
//! exposes the **monolingual** subset (entries where source language equals
//! target language, e.g. `en → en`). Bilingual pairs are ignored.

use std::collections::HashMap;

use chrono::NaiveDate;
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

    /// Date of the last release.
    #[serde(with = "date_serde")]
    pub updated: NaiveDate,

    /// Number of headword entries in the dictionary.
    pub words: u64,

    /// English name of the dictionary (e.g. `"English - French"`).
    ///
    /// Doubles as the Fluent message ID for the localised name, see
    /// [`dictionary_label`]. `None` for metadata cached before the upstream API
    /// shipped the field.
    pub name: Option<String>,

    /// Name of the dictionary with every language written in its own language
    /// (e.g. `"français"`). Never translated.
    #[serde(rename = "name-loc")]
    pub name_loc: Option<String>,
}

/// Builds the user-facing name of a dictionary.
///
/// Renders the Cadmus-localised name and the native [`DictionaryEntry::name_loc`]
/// side by side (`"French | français"`), collapsing to whichever half
/// carries information. Falls back to the bare `lang` code for an absent entry
/// or names that are missing or empty.
pub(crate) fn dictionary_label(lang: &str, entry: Option<&DictionaryEntry>) -> String {
    use crate::fl_or;

    let native = entry
        .and_then(|e| e.name_loc.as_deref())
        .filter(|s| !s.is_empty());
    let name = entry
        .and_then(|e| e.name.as_deref())
        .filter(|s| !s.is_empty());

    match (name, native) {
        (None, None) => lang.to_owned(),
        (None, Some(native)) => native.to_owned(),
        (Some(name), native) => {
            let fallback = native.unwrap_or(name);
            let localised = fl_or!(name.to_lowercase(), fallback);
            match native {
                Some(native) if localised != native => format!("{localised} | {native}"),
                Some(native) => native.to_owned(),
                None => localised.to_owned(),
            }
        }
    }
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

mod date_serde {
    use chrono::NaiveDate;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    const FORMAT: &str = "%Y-%m-%d";

    pub fn serialize<S>(date: &NaiveDate, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        date.format(FORMAT).to_string().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveDate, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        NaiveDate::parse_from_str(&s, FORMAT).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fl;

    fn make_entry() -> DictionaryEntry {
        DictionaryEntry {
            formats: "df,dic,dictorg,kobo,mobi,stardict".to_string(),
            updated: NaiveDate::from_ymd_opt(2026, 4, 1).unwrap(),
            words: 1_381_375,
            name: Some("English".to_string()),
            name_loc: Some("English".to_string()),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_download_url_english() {
        assert_eq!(
            download_url("en"),
            "https://www.reader-dict.com/file/en/dictorg-en-en.zip"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_download_url_no_etym_english() {
        assert_eq!(
            download_url_no_etym("en"),
            "https://www.reader-dict.com/file/en/dictorg-en-en-noetym.zip"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_download_url_french() {
        assert_eq!(
            download_url("fr"),
            "https://www.reader-dict.com/file/fr/dictorg-fr-fr.zip"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_deserialize_response() {
        let json = r#"{
            "en": {
                "en": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "name": "English", "name-loc": "English", "updated": "2026-04-01", "words": 1381375 },
                "fr": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "name": "English - French", "name-loc": "English - français", "updated": "2026-04-01", "words": 50000 }
            },
            "fr": {
                "fr": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "name": "French", "name-loc": "français", "updated": "2026-03-01", "words": 2050655 }
            }
        }"#;

        let resp: DictionariesResponse = serde_json::from_str(json).unwrap();

        let en_entry = resp.get("en").and_then(|m| m.get("en")).unwrap();
        assert_eq!(en_entry.words, 1_381_375);
        assert_eq!(
            en_entry.updated,
            NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()
        );

        let fr_entry = resp.get("fr").and_then(|m| m.get("fr")).unwrap();
        assert_eq!(fr_entry.words, 2_050_655);

        assert_eq!(*en_entry, make_entry());
    }

    #[test]
    fn test_deserialize_without_names_yields_none() {
        let json = r#"{
            "en": {
                "en": { "formats": "dictorg", "updated": "2026-04-01", "words": 1381375 }
            }
        }"#;

        let resp: DictionariesResponse = serde_json::from_str(json).unwrap();
        let entry = resp.get("en").and_then(|m| m.get("en")).unwrap();
        assert_eq!(entry.name.as_deref(), None);
        assert_eq!(entry.name_loc.as_deref(), None);
    }

    #[test]
    fn test_dictionary_label_without_entry_is_the_lang() {
        assert_eq!(dictionary_label("en", None), "en");
    }

    #[test]
    fn test_dictionary_label_dedupes_identical_names() {
        assert_eq!(dictionary_label("en", Some(&make_entry())), "English");
    }

    #[test]
    fn test_dictionary_label_falls_back_when_names_missing() {
        let entry = DictionaryEntry {
            name: None,
            name_loc: None,
            ..make_entry()
        };
        assert_eq!(dictionary_label("fr", Some(&entry)), "fr");

        let entry = DictionaryEntry {
            name: None,
            name_loc: Some(String::new()),
            ..make_entry()
        };
        assert_eq!(dictionary_label("fr", Some(&entry)), "fr");
    }

    #[test]
    fn test_dictionary_label_without_english_name_is_native_only() {
        let entry = DictionaryEntry {
            name: None,
            name_loc: Some("français".to_string()),
            ..make_entry()
        };
        assert_eq!(dictionary_label("fr", Some(&entry)), "français");
    }

    #[test]
    fn test_dictionary_label_joins_translation_with_native_name() {
        let entry = DictionaryEntry {
            name: Some("French".to_string()),
            name_loc: Some("français".to_string()),
            ..make_entry()
        };
        let expected = format!("{} | {}", fl!("french"), "français");
        assert_eq!(dictionary_label("fr", Some(&entry)), expected);
    }

    #[test]
    fn test_dictionary_label_omits_lang_code_when_native_name_missing() {
        let entry = DictionaryEntry {
            name: Some("French".to_string()),
            name_loc: None,
            ..make_entry()
        };
        assert_eq!(dictionary_label("fr", Some(&entry)), fl!("french"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_monolingual_filter() {
        let json = r#"{
            "en": {
                "en": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "name": "English", "name-loc": "English", "updated": "2026-04-01", "words": 1381375 },
                "fr": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "name": "English - French", "name-loc": "English - français", "updated": "2026-04-01", "words": 50000 }
            },
            "af": {
                "en": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "name": "Afrikaans - English", "name-loc": "Afrikaans - English", "updated": "2026-04-01", "words": 8934 }
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
