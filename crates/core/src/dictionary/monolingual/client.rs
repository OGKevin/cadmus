//! HTTP client for monolingual dictionary API operations.
//!
//! Only monolingual dictionaries (source language == target language) are
//! supported. Bilingual pairs present in the API response are ignored.

use super::db::Db;
use super::errors::MonolingualError;
use super::metadata::{DictionariesResponse, DictionaryEntry};
use crate::db::Database;
use crate::http::Client;

const MONOLINGUAL_API_URL: &str = "https://www.reader-dict.com/api/v1/dictionaries";

/// Monolingual dictionary HTTP client.
///
/// This client queries the monolingual API and manages a SQLite cache of
/// dictionary metadata. It composes the base [`crate::http::Client`] for all
/// network requests.
#[derive(Clone)]
pub(super) struct MonolingualClient {
    http: Client,
    db: Db,
}

impl std::fmt::Debug for MonolingualClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonolingualClient")
            .field("db", &self.db)
            .finish_non_exhaustive()
    }
}

impl MonolingualClient {
    /// Creates a new monolingual client backed by the given database.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client fails to build.
    #[cfg_attr(feature = "otel", tracing::instrument(skip_all))]
    pub(super) fn new(database: &Database) -> Result<Self, MonolingualError> {
        tracing::debug!("Building monolingual client");

        let http = Client::new()?;
        let db = Db::new(database);

        tracing::debug!("Monolingual client built successfully");
        Ok(Self { http, db })
    }

    /// Fetches dictionary metadata from the API and caches the response.
    ///
    /// If the metadata is already cached, this will overwrite it with the
    /// latest from the API. For offline availability, use
    /// [`Self::get_cached_metadata`] instead.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be
    /// parsed.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    fn fetch_metadata(&self) -> Result<DictionariesResponse, MonolingualError> {
        tracing::debug!("Fetching monolingual metadata from API");

        let text = self
            .http
            .get(MONOLINGUAL_API_URL)
            .send()
            .map_err(|e| MonolingualError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| MonolingualError::Request(e.to_string()))?
            .text()
            .map_err(|e| MonolingualError::Request(e.to_string()))?;

        let metadata: DictionariesResponse = serde_json::from_str(&text)?;

        for (source_lang, targets) in &metadata {
            if let Some(entry) = targets.get(source_lang.as_str()) {
                self.db.upsert_entry(source_lang, entry)?;
            }
        }

        tracing::debug!("Cached monolingual metadata to database");
        Ok(metadata)
    }

    /// Gets cached dictionary metadata if available.
    ///
    /// This does not make any network requests and is suitable for offline
    /// use. Returns `None` if no entries have been cached yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the database read fails.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    fn get_cached_metadata(&self) -> Result<Option<DictionariesResponse>, MonolingualError> {
        let entries = self.db.get_all_entries()?;

        if entries.is_empty() {
            tracing::debug!("No cached metadata found in database");
            return Ok(None);
        }

        let mut response = DictionariesResponse::new();
        for (lang, entry) in entries {
            response
                .entry(lang.clone())
                .or_default()
                .insert(lang, entry);
        }

        tracing::debug!("Loaded cached metadata from database");
        Ok(Some(response))
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    fn load_metadata(&self) -> Result<DictionariesResponse, MonolingualError> {
        match self.get_cached_metadata()? {
            Some(metadata) => Ok(metadata),
            None => {
                tracing::debug!("Cache miss, fetching from API");
                self.fetch_metadata()
            }
        }
    }

    /// Returns all available monolingual dictionaries.
    ///
    /// Only entries where source language equals target language are returned.
    /// Bilingual pairs present in the API response are ignored.
    ///
    /// First tries to load from cache, falls back to an API fetch if not
    /// cached.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata cannot be loaded.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub(super) fn get_available_dictionaries(
        &self,
    ) -> Result<Vec<(String, DictionaryEntry)>, MonolingualError> {
        let metadata = self.load_metadata()?;

        let monolingual = metadata
            .into_iter()
            .filter_map(|(lang, mut targets)| targets.remove(&lang).map(|entry| (lang, entry)))
            .collect();

        Ok(monolingual)
    }

    /// Downloads raw bytes from `url`.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(url = %url)))]
    pub(super) fn download_bytes(&self, url: &str) -> Result<Vec<u8>, MonolingualError> {
        tracing::debug!(url = %url, "Downloading bytes");

        let bytes = self
            .http
            .get(url)
            .send()
            .map_err(|e| MonolingualError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| MonolingualError::Request(e.to_string()))?
            .bytes()
            .map_err(|e| MonolingualError::Request(e.to_string()))?;

        tracing::debug!(url = %url, bytes = bytes.len(), "Download complete");

        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn create_test_client() -> (MonolingualClient, Database) {
        crate::crypto::init_crypto_provider();
        let database = Database::new(":memory:").expect("failed to create in-memory database");
        database.migrate().expect("failed to run migrations");
        let client = MonolingualClient::new(&database).expect("failed to create client");
        (client, database)
    }

    fn make_response_json() -> String {
        r#"{
            "en": {
                "en": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-04-01", "words": 1381375 },
                "fr": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-04-01", "words": 50000 }
            },
            "fr": {
                "fr": { "formats": "df,dic,dictorg,kobo,mobi,stardict", "updated": "2026-03-01", "words": 2050655 }
            }
        }"#.to_string()
    }

    #[test]
    fn test_client_creation() {
        let (_client, _database) = create_test_client();
    }

    #[test]
    fn test_no_cache_initially() {
        let (client, _database) = create_test_client();
        let result = client.get_cached_metadata().expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_roundtrip() {
        let (client, _database) = create_test_client();

        let json = make_response_json();
        let metadata: DictionariesResponse = serde_json::from_str(&json).unwrap();
        for (source_lang, targets) in &metadata {
            if let Some(entry) = targets.get(source_lang.as_str()) {
                client.db.upsert_entry(source_lang, entry).unwrap();
            }
        }

        let cached = client.get_cached_metadata().expect("should load cache");
        assert!(cached.is_some());
        let cached = cached.unwrap();
        assert!(cached.contains_key("en"));
        assert!(cached["en"].contains_key("en"));
        assert_eq!(cached["en"]["en"].words, 1_381_375);
    }

    #[test]
    fn test_get_available_dictionaries_filters_bilingual() {
        let (client, _database) = create_test_client();

        let json = make_response_json();
        let metadata: DictionariesResponse = serde_json::from_str(&json).unwrap();
        for (source_lang, targets) in &metadata {
            if let Some(entry) = targets.get(source_lang.as_str()) {
                client.db.upsert_entry(source_lang, entry).unwrap();
            }
        }

        let available = client.get_available_dictionaries().unwrap();
        // "en→fr" bilingual entry must be excluded; only "en→en" and "fr→fr" returned
        assert_eq!(available.len(), 2);
        let langs: Vec<&str> = available.iter().map(|(l, _)| l.as_str()).collect();
        assert!(langs.contains(&"en"));
        assert!(langs.contains(&"fr"));
    }

    /// Fetches live metadata from the monolingual API.
    ///
    /// Run with: `cargo test -- --ignored`
    #[test]
    #[ignore = "requires network access to www.reader-dict.com"]
    fn test_fetch_metadata_live() {
        let (client, _database) = create_test_client();
        let result = client.fetch_metadata();
        assert!(result.is_ok(), "fetch_metadata failed: {:?}", result.err());
        let metadata = result.unwrap();
        assert!(
            !metadata.is_empty(),
            "Expected at least one language in response"
        );
        assert!(
            metadata.get("en").and_then(|m| m.get("en")).is_some(),
            "Expected English monolingual dictionary in response"
        );
    }

    /// Fetches metadata and verifies the entry is written to the database.
    ///
    /// Run with: `cargo test -- --ignored`
    #[test]
    #[ignore = "requires network access to www.reader-dict.com"]
    fn test_fetch_metadata_writes_cache() {
        let (client, _database) = create_test_client();
        client.fetch_metadata().expect("fetch_metadata failed");
        let cached = client
            .get_cached_metadata()
            .expect("get_cached_metadata failed");
        assert!(
            cached.is_some(),
            "Expected cache to be populated after fetch"
        );
    }
}
