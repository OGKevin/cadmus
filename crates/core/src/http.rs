//! Reusable HTTP client with pre-configured TLS, timeouts, and user agent.
//!
//! This module provides [`Client`] as the recommended base HTTP client for all
//! network requests in the application. It is pre-configured with:
//!
//! - TLS using `webpki-roots` certificates (no system cert store required)
//! - 30 second request timeout
//! - User agent identifying the application
//!
//! # Example
//!
//! ```no_run
//! use cadmus_core::http::Client;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = Client::new()?;
//!     client.get("https://example.com").send()?;
//!     Ok(())
//! }
//! ```

use reqwest::blocking::{Client as ReqwestClient, RequestBuilder};
use rustls::RootCertStore;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

pub const CLIENT_TIMEOUT_SECS: u64 = 30;

const USER_AGENT: &str = concat!("github.com/OGKevin/cadmus/", env!("GIT_VERSION"));

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("Failed to build HTTP client: {0}")]
    Build(#[from] reqwest::Error),
}

const MIN_CHUNK_SIZE: usize = 256 * 1024;
const MAX_CHUNK_SIZE: usize = 10 * 1024 * 1024;
const INITIAL_CHUNK_SIZE: usize = 1024 * 1024;
/// Target 80% of the HTTP timeout to leave headroom for throughput variance.
const TARGET_CHUNK_SECS: f64 = CLIENT_TIMEOUT_SECS as f64 * 0.8;
const MAX_RETRIES: usize = 3;

/// Error types that can occur during a chunked HTTP download.
#[derive(Error, Debug)]
pub enum ChunkedDownloadError {
    #[error("HTTP request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Pre-configured HTTP client for making network requests.
///
/// This client should be used as the base for all HTTP requests rather than
/// constructing raw `reqwest` clients. It comes with:
/// - TLS using `webpki-roots` certificates (works on Kobo devices without system cert store)
/// - 30 second request timeout
/// - User agent header set
///
/// # Example
///
/// ```no_run
/// use cadmus_core::http::Client;
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let client = Client::new()?;
///     client.get("https://api.github.com").send()?;
///     Ok(())
/// }
/// ```
pub struct Client {
    client: ReqwestClient,
}

impl Client {
    pub fn new() -> Result<Self, HttpError> {
        let root_store = build_root_store();

        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let client = ReqwestClient::builder()
            .use_preconfigured_tls(tls_config)
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(CLIENT_TIMEOUT_SECS))
            .build()
            .map_err(HttpError::Build)?;

        tracing::debug!("HTTP client built successfully");
        Ok(Self { client })
    }

    pub fn get(&self, url: &str) -> RequestBuilder {
        self.client.get(url)
    }

    pub fn post(&self, url: &str) -> RequestBuilder {
        self.client.post(url)
    }

    /// Downloads a file to `dest` using HTTP Range requests.
    ///
    /// `request_builder` is called once per chunk (and per retry) to produce a
    /// `RequestBuilder` for the given URL. The caller is responsible for adding
    /// any required headers (e.g. `Authorization`).
    ///
    /// `progress_callback` is called after each successful chunk with
    /// `(bytes_downloaded_so_far, total_bytes)`.
    ///
    /// # Errors
    ///
    /// Returns `ChunkedDownloadError::Io` if the destination file cannot be created
    /// or written. Returns `ChunkedDownloadError::Request` if all retry attempts for
    /// any chunk fail.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cadmus_core::http::Client;
    /// use std::path::PathBuf;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = Client::new()?;
    /// let dest = PathBuf::from("/tmp/downloaded_file");
    ///
    /// client.download(
    ///     "https://example.com/large-file.bin",
    ///     1024 * 1024,
    ///     &dest,
    ///     |url| client.get(url),
    ///     &mut |downloaded, total| println!("{}/{}", downloaded, total),
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(skip(self, request_builder, progress_callback))
    )]
    pub fn download<B, F>(
        &self,
        url: &str,
        total_size: u64,
        dest: &PathBuf,
        request_builder: B,
        progress_callback: &mut F,
    ) -> Result<(), ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
        F: FnMut(u64, u64),
    {
        progress_callback(0, total_size);

        tracing::debug!(url = %url, "Downloading file");
        tracing::debug!(path = ?dest, "Download destination");

        let mut file = std::fs::File::create(dest)?;

        let mut downloaded = 0u64;
        let mut chunk_size = INITIAL_CHUNK_SIZE;

        tracing::debug!(
            initial_chunk_size = INITIAL_CHUNK_SIZE,
            "Starting chunked download"
        );

        while downloaded < total_size {
            let chunk_start = downloaded;
            let chunk_end = std::cmp::min(downloaded + chunk_size as u64 - 1, total_size - 1);

            tracing::debug!(
                chunk_start,
                chunk_end,
                chunk_size,
                total_size,
                "Downloading chunk"
            );

            let start = std::time::Instant::now();
            let chunk_data =
                Self::download_chunk_with_retries(url, chunk_start, chunk_end, &request_builder)?;
            let elapsed_secs = start.elapsed().as_secs_f64();

            file.write_all(&chunk_data)?;
            downloaded += chunk_data.len() as u64;

            if elapsed_secs > 0.0 {
                let throughput = chunk_data.len() as f64 / elapsed_secs;
                chunk_size = ((throughput * TARGET_CHUNK_SECS) as usize)
                    .clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE);
                tracing::debug!(
                    elapsed_secs,
                    throughput_bytes_per_sec = throughput as u64,
                    next_chunk_size = chunk_size,
                    "Adjusted chunk size"
                );
            }

            progress_callback(downloaded, total_size);

            tracing::debug!(
                downloaded,
                total_size,
                progress_percent = (downloaded as f64 / total_size as f64) * 100.0,
                "Download progress"
            );
        }

        tracing::debug!(bytes = downloaded, "Download complete");
        tracing::debug!(path = ?dest, "Saved file");

        Ok(())
    }

    /// Downloads a specific byte range with automatic exponential-backoff retry.
    ///
    /// # Errors
    ///
    /// Returns an error if all `MAX_RETRIES` attempts fail.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(request_builder)))]
    fn download_chunk_with_retries<B>(
        url: &str,
        start: u64,
        end: u64,
        request_builder: &B,
    ) -> Result<Vec<u8>, ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
    {
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match Self::download_chunk(url, start, end, request_builder) {
                Ok(data) => {
                    if attempt > 1 {
                        tracing::debug!(
                            attempt,
                            max_retries = MAX_RETRIES,
                            "Chunk download succeeded after retry"
                        );
                    }
                    return Ok(data);
                }
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        max_retries = MAX_RETRIES,
                        error = %e,
                        "Chunk download failed"
                    );
                    last_error = Some(e);

                    if attempt < MAX_RETRIES {
                        let backoff_ms = 1000 * (2u64.pow(attempt as u32 - 1));
                        tracing::debug!(backoff_ms, "Retrying after backoff");
                        std::thread::sleep(Duration::from_millis(backoff_ms));
                    }
                }
            }
        }

        Err(last_error.expect("MAX_RETRIES >= 1, so last_error is always set"))
    }

    /// Downloads a specific byte range from a URL using the HTTP `Range` header.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns a non-2xx status.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(request_builder)))]
    fn download_chunk<B>(
        url: &str,
        start: u64,
        end: u64,
        request_builder: &B,
    ) -> Result<Vec<u8>, ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
    {
        let range_header = format!("bytes={}-{}", start, end);

        let bytes = request_builder(url)
            .header("Range", range_header)
            .send()?
            .error_for_status()?
            .bytes()?;

        Ok(bytes.to_vec())
    }
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
        }
    }
}

fn build_root_store() -> RootCertStore {
    let mut store = RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    store
}
