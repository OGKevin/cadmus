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

use backon::{BackoffBuilder, ExponentialBuilder};
use reqwest::blocking::{Client as ReqwestClient, RequestBuilder};
use rustls::RootCertStore;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use thiserror::Error;

pub const CLIENT_TIMEOUT_SECS: u64 = 30;

const USER_AGENT: &str = concat!("github.com/OGKevin/cadmus/", env!("GIT_VERSION"));

const CANCEL_STATE_RUNNING: u8 = 0;
const CANCEL_STATE_CANCELLED: u8 = 1;
const CANCEL_STATE_COMMITTED: u8 = 2;

/// Shared cancel/commit gate for long-running downloads and deploys.
///
/// Cancellation wins until [`Self::try_commit`] succeeds. After a successful
/// commit transition, further cancel requests are ignored.
#[derive(Debug, Default)]
pub struct CancelFlag {
    state: AtomicU8,
}

impl CancelFlag {
    /// Creates a running cancel flag.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(CANCEL_STATE_RUNNING),
        }
    }

    /// Requests cancellation. No-op if the operation already committed.
    pub fn request_cancel(&self) {
        let _ = self.state.compare_exchange(
            CANCEL_STATE_RUNNING,
            CANCEL_STATE_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Returns `true` when cancellation won before commit.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == CANCEL_STATE_CANCELLED
    }

    /// Atomically commits if still running.
    ///
    /// Returns `true` when this call commits the operation. Returns `false`
    /// when cancellation already won or another caller already committed.
    #[must_use]
    pub fn try_commit(&self) -> bool {
        self.state
            .compare_exchange(
                CANCEL_STATE_RUNNING,
                CANCEL_STATE_COMMITTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

/// Pollable cancel check for long-running downloads and deploys.
///
/// Prefer [`Self::from_flag`] with a shared [`CancelFlag`] so deploy can
/// atomically choose between cancellation and publishing. [`Self::new`] wraps a
/// plain predicate for tests and call sites that only need polling.
#[derive(Clone, Copy)]
pub enum CancelFunc<'a> {
    /// Never cancels and always allows commit.
    Never,
    /// Poll-only cancel predicate without an atomic commit transition.
    Check(&'a dyn Fn() -> bool),
    /// Shared cancel/commit gate.
    Flag(&'a CancelFlag),
}

impl std::fmt::Debug for CancelFunc<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CancelFunc(..)")
    }
}

impl<'a> CancelFunc<'a> {
    /// Wraps a cancel predicate that returns `true` when work should stop.
    #[must_use]
    pub const fn new(check: &'a dyn Fn() -> bool) -> Self {
        Self::Check(check)
    }

    /// Wraps a shared [`CancelFlag`] that supports atomic commit.
    #[must_use]
    pub const fn from_flag(flag: &'a CancelFlag) -> Self {
        Self::Flag(flag)
    }

    /// A cancel check that never requests cancellation.
    #[must_use]
    pub const fn never() -> CancelFunc<'static> {
        CancelFunc::Never
    }

    /// Returns `true` when the operation should abort.
    #[must_use]
    pub fn is_cancelled(self) -> bool {
        match self {
            Self::Never => false,
            Self::Check(check) => check(),
            Self::Flag(flag) => flag.is_cancelled(),
        }
    }

    /// Atomically commits when backed by a [`CancelFlag`]; otherwise re-checks
    /// cancellation. Returns `false` when cancellation already won.
    #[must_use]
    pub fn try_commit(self) -> bool {
        match self {
            Self::Never => true,
            Self::Check(check) => !check(),
            Self::Flag(flag) => flag.try_commit(),
        }
    }
}

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
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Error types that can occur during a chunked HTTP download.
#[derive(Error, Debug)]
pub enum ChunkedDownloadError {
    #[error("HTTP request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Download cancelled")]
    Cancelled,
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

    pub fn head(&self, url: &str) -> RequestBuilder {
        self.client.head(url)
    }

    pub fn get(&self, url: &str) -> RequestBuilder {
        self.client.get(url)
    }

    pub fn post(&self, url: &str) -> RequestBuilder {
        self.client.post(url)
    }

    /// Returns the inner `reqwest::blocking::Client` for use with third-party
    /// libraries that require a raw client (e.g. pyroscope-rs).
    pub fn into_reqwest(self) -> ReqwestClient {
        self.client
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
    /// any chunk fail. Returns `ChunkedDownloadError::Cancelled` when `should_cancel`
    /// reports cancellation, including between retry attempts and during backoff.
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
    ///     None,
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, request_builder, progress_callback))
    )]
    pub fn download<B, F>(
        &self,
        url: &str,
        total_size: u64,
        dest: &PathBuf,
        request_builder: B,
        progress_callback: &mut F,
        should_cancel: Option<CancelFunc<'_>>,
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
            if should_cancel.is_some_and(CancelFunc::is_cancelled) {
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(ChunkedDownloadError::Cancelled);
            }

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
            let chunk_data = match Self::download_chunk_with_retries(
                url,
                chunk_start,
                chunk_end,
                &request_builder,
                should_cancel,
            ) {
                Ok(data) => data,
                Err(ChunkedDownloadError::Cancelled) => {
                    drop(file);
                    let _ = std::fs::remove_file(dest);
                    return Err(ChunkedDownloadError::Cancelled);
                }
                Err(e) => return Err(e),
            };
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
    /// Returns an error if all retry attempts fail, or
    /// [`ChunkedDownloadError::Cancelled`] if cancellation is requested before a
    /// retry or during backoff.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(request_builder, should_cancel))
    )]
    fn download_chunk_with_retries<B>(
        url: &str,
        start: u64,
        end: u64,
        request_builder: &B,
        should_cancel: Option<CancelFunc<'_>>,
    ) -> Result<Vec<u8>, ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
    {
        let mut backoff = ExponentialBuilder::default()
            .with_min_delay(Duration::from_secs(1))
            .with_factor(2.0)
            .with_max_times(MAX_RETRIES.saturating_sub(1))
            .build();
        let mut attempt = 0_u32;

        loop {
            if should_cancel.is_some_and(CancelFunc::is_cancelled) {
                return Err(ChunkedDownloadError::Cancelled);
            }

            attempt += 1;
            match Self::download_chunk(url, start, end, request_builder) {
                Ok(data) => {
                    if attempt > 1 {
                        tracing::debug!(attempt, "Chunk download succeeded after retry");
                    }
                    return Ok(data);
                }
                Err(ChunkedDownloadError::Cancelled) => {
                    return Err(ChunkedDownloadError::Cancelled);
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "Chunk download failed");
                    match backoff.next() {
                        Some(delay) => {
                            tracing::debug!(
                                backoff_ms = delay.as_millis(),
                                "Retrying after backoff"
                            );
                            sleep_interruptible(delay, should_cancel)?;
                        }
                        None => return Err(e),
                    }
                }
            }
        }
    }

    /// Downloads a specific byte range from a URL using the HTTP `Range` header.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns a non-2xx status.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(request_builder)))]
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

fn sleep_interruptible(
    duration: Duration,
    should_cancel: Option<CancelFunc<'_>>,
) -> Result<(), ChunkedDownloadError> {
    let Some(should_cancel) = should_cancel else {
        std::thread::sleep(duration);
        return Ok(());
    };

    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        if should_cancel.is_cancelled() {
            return Err(ChunkedDownloadError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        std::thread::sleep(remaining.min(CANCEL_POLL_INTERVAL));
    }

    if should_cancel.is_cancelled() {
        return Err(ChunkedDownloadError::Cancelled);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_flag_cancel_wins_before_commit() {
        let flag = CancelFlag::new();
        flag.request_cancel();
        assert!(flag.is_cancelled());
        assert!(!flag.try_commit());
    }

    #[test]
    fn cancel_flag_commit_wins_over_late_cancel() {
        let flag = CancelFlag::new();
        assert!(flag.try_commit());
        flag.request_cancel();
        assert!(!flag.is_cancelled());
        assert!(!flag.try_commit());
    }

    #[test]
    fn download_returns_cancelled_before_first_chunk() {
        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-cancel-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("partial.bin");

        let cancel_check = || true;
        let result = client.download(
            "https://example.invalid/unused",
            1024,
            &dest,
            |url| client.get(url),
            &mut |_, _| {},
            Some(CancelFunc::new(&cancel_check)),
        );

        assert!(matches!(result, Err(ChunkedDownloadError::Cancelled)));
        assert!(!dest.exists(), "partial download file should be removed");
    }

    #[test]
    fn download_returns_cancelled_during_chunk_retries() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-cancel-retry-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("partial.bin");
        let checks = AtomicUsize::new(0);
        let cancel_check = || checks.fetch_add(1, Ordering::Relaxed) >= 2;

        let result = client.download(
            "http://127.0.0.1:1/unused",
            1024,
            &dest,
            |url| client.get(url),
            &mut |_, _| {},
            Some(CancelFunc::new(&cancel_check)),
        );

        assert!(matches!(result, Err(ChunkedDownloadError::Cancelled)));
        assert!(!dest.exists(), "partial download file should be removed");
    }
}
