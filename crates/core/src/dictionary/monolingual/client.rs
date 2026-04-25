//! HTTP client for monolingual dictionary API operations.
//!
//! Only monolingual dictionaries (source language == target language) are
//! supported. Bilingual pairs present in the API response are ignored.

use super::errors::MonolingualError;
use super::metadata::DictionariesResponse;
use crate::db::types::UnixTimestamp;
use crate::http::Client;
use chrono::DateTime;

const MONOLINGUAL_API_URL: &str = "https://www.reader-dict.com/api/v1/dictionaries";

/// Monolingual dictionary HTTP client.
///
/// Handles all network operations: fetching the remote metadata catalogue and
/// downloading dictionary archives. All persistence is handled by the service
/// layer; this type carries no database state.
#[derive(Clone)]
pub(super) struct MonolingualClient {
    http: Client,
}

impl std::fmt::Debug for MonolingualClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonolingualClient").finish_non_exhaustive()
    }
}

impl MonolingualClient {
    /// Creates a new monolingual HTTP client.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client fails to build.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub(super) fn new() -> Result<Self, MonolingualError> {
        tracing::debug!("Building monolingual client");
        let http = Client::new()?;
        tracing::debug!("Monolingual client built successfully");
        Ok(Self { http })
    }

    /// Fetches dictionary metadata from the remote API and returns the parsed
    /// response.
    ///
    /// The caller is responsible for persisting the result to the database.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or the response cannot be
    /// parsed.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub(super) fn fetch_metadata(&self) -> Result<DictionariesResponse, MonolingualError> {
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

        tracing::debug!("Fetched monolingual metadata from API");
        Ok(metadata)
    }

    /// Sends a HEAD request with `If-Modified-Since: <since>` and returns
    /// `false` if the server responds 304 (cache still valid) or `true` if
    /// the server responds 200 (new data available).
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or returns an unexpected
    /// status code.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(since = %since), ret(level=tracing::Level::TRACE)))]
    pub(super) fn is_metadata_modified_since(
        &self,
        since: UnixTimestamp,
    ) -> Result<bool, MonolingualError> {
        let since_str = DateTime::from(since)
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();

        let response = self
            .http
            .head(MONOLINGUAL_API_URL)
            .header("If-Modified-Since", &since_str)
            .send()
            .map_err(|e| MonolingualError::Request(e.to_string()))?;

        match response.status() {
            reqwest::StatusCode::NOT_MODIFIED => Ok(false),
            s if s.is_success() => Ok(true),
            s => Err(MonolingualError::Request(format!("unexpected status: {s}"))),
        }
    }

    /// Downloads `url` to `dest` using chunked HTTP Range requests.
    ///
    /// Issues a minimal `bytes=0-0` Range request first to read `Content-Range`
    /// and obtain the total file size, then delegates to the chunked
    /// [`crate::http::Client::download`] method which handles retries and
    /// adaptive chunk sizing.
    ///
    /// `progress_callback` receives `(bytes_downloaded_so_far, total_bytes)`
    /// after each chunk.
    ///
    /// # Errors
    ///
    /// Returns an error if the probe request fails or returns a non-2xx
    /// status, if the `Content-Range` header is missing, or if the chunked
    /// download fails.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, progress_callback), fields(url = %url)))]
    pub(super) fn download<F>(
        &self,
        url: &str,
        dest: &std::path::Path,
        progress_callback: &mut F,
    ) -> Result<(), MonolingualError>
    where
        F: FnMut(u64, u64),
    {
        tracing::debug!(url = %url, "Probing content length");

        let response = self
            .http
            .head(url)
            .header("Range", "bytes=0-0")
            .send()
            .map_err(|e| MonolingualError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| MonolingualError::Request(e.to_string()))?;

        let total_size = response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split('/').next_back())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| MonolingualError::Request("Missing Content-Range header".to_string()))?;

        tracing::debug!(url = %url, total_size, "Starting chunked download");

        self.http
            .download(
                url,
                total_size,
                &dest.to_path_buf(),
                |u| self.http.get(u),
                progress_callback,
            )
            .map_err(|e| MonolingualError::Request(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_client() -> MonolingualClient {
        crate::crypto::init_crypto_provider();
        MonolingualClient::new().expect("failed to create client")
    }

    #[test]
    fn test_client_creation() {
        create_test_client();
    }

    /// Fetches live metadata from the monolingual API.
    ///
    /// Run with: `cargo test -- --ignored`
    #[test]
    #[ignore = "requires network access to www.reader-dict.com"]
    fn test_fetch_metadata_live() {
        let client = create_test_client();
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

    #[test]
    #[ignore = "requires network access to www.reader-dict.com"]
    fn test_is_metadata_modified_since() {
        let client = create_test_client();
        let old_ts = UnixTimestamp::from(chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap());
        let result = client.is_metadata_modified_since(old_ts);
        assert!(result.is_ok());
        assert!(
            result.unwrap(),
            "expected server to report modified since year 2000"
        );
    }
}
