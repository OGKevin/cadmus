//! Reusable HTTP client with pre-configured TLS, timeouts, and user agent.
//!
//! This module provides [`Client`] as the recommended base HTTP client for all
//! network requests in the application. It is pre-configured with:
//!
//! - TLS using `webpki-roots` certificates (no system cert store required)
//! - Connect timeout and total request timeout (see [`CLIENT_CONNECT_TIMEOUT_SECS`]
//!   / [`CLIENT_TIMEOUT_SECS`])
//! - User agent identifying the application
//! - Retries for transient failures (connection errors, timeouts, HTTP 408,
//!   429, and 5xx): three attempts, starting at 1s, doubling, no jitter;
//!   `Retry-After` on successful responses is honoured (capped)
//! - A tracing span per request attempt when the `tracing` feature is enabled
//!
//! Middleware retries wrap `send()` (headers). They do **not** retry mid-body
//! read failures; see [`Client::download`].
//!
//! # Example
//!
//! ```no_run
//! use cadmus_core::http::Client;
//!
//! async fn example() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = Client::new()?;
//!     client.get("https://example.com").send().await?;
//!     Ok(())
//! }
//! ```

mod retry;

use reqwest::Client as ReqwestClient;
use reqwest_middleware::{ClientWithMiddleware, RequestBuilder};
use rustls::RootCertStore;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncWriteExt;

use retry::client_with_retry;

pub const CLIENT_TIMEOUT_SECS: u64 = 30;
pub const CLIENT_CONNECT_TIMEOUT_SECS: u64 = 10;

pub(crate) const USER_AGENT: &str = concat!("github.com/OGKevin/cadmus/", env!("GIT_VERSION"));

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
    Check(&'a (dyn Fn() -> bool + Send + Sync)),
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
    pub const fn new(check: &'a (dyn Fn() -> bool + Send + Sync)) -> Self {
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
/// Sleeps after a failed attempt. Two sleeps means three attempts in total.
pub(crate) const RETRY_BACKOFFS: usize = 2;

/// Error types that can occur during a chunked HTTP download.
#[derive(Error, Debug)]
pub enum ChunkedDownloadError {
    #[error("HTTP request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Download cancelled")]
    Cancelled,
    #[error("HTTP request error: {0}")]
    Failed(String),
}

impl From<reqwest_middleware::Error> for ChunkedDownloadError {
    fn from(error: reqwest_middleware::Error) -> Self {
        match reqwest_error(error) {
            Ok(error) => Self::Request(error),
            Err(message) => Self::Failed(message),
        }
    }
}

/// Pre-configured HTTP client for making network requests.
///
/// This client should be used as the base for all HTTP requests rather than
/// constructing raw `reqwest` clients. It comes with:
/// - TLS using `webpki-roots` certificates (works on Kobo devices without system cert store)
/// - Connect and total request timeouts
/// - User agent header set
/// - Transient-failure retries on every request (header/`send()` level), honouring
///   `Retry-After` when present
/// - Request spans when the `tracing` feature is enabled
///
/// # Example
///
/// ```no_run
/// use cadmus_core::http::Client;
///
/// async fn example() -> Result<(), Box<dyn std::error::Error>> {
///     let client = Client::new()?;
///     client.get("https://api.github.com").send().await?;
///     Ok(())
/// }
/// ```
pub struct Client {
    raw: ReqwestClient,
    client: ClientWithMiddleware,
}

impl Client {
    pub fn new() -> Result<Self, HttpError> {
        let root_store = build_root_store();

        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let raw = ReqwestClient::builder()
            .use_preconfigured_tls(tls_config)
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(CLIENT_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(CLIENT_TIMEOUT_SECS))
            .build()
            .map_err(HttpError::Build)?;
        let client = client_with_retry(raw.clone());

        tracing::debug!("HTTP client built successfully");
        Ok(Self { raw, client })
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

    /// Returns the middleware client used for application requests.
    ///
    /// Includes transient retries and, when the `tracing` feature is enabled,
    /// a span per attempt. The raw client from [`Self::into_reqwest`] does not.
    pub(crate) fn middleware(&self) -> ClientWithMiddleware {
        self.client.clone()
    }

    /// Returns the inner [`reqwest::Client`] for libraries that take one directly,
    /// such as the OpenTelemetry exporter.
    ///
    /// The returned client has neither retry nor request-span middleware, so
    /// export traffic does not retry through the application policy or trace
    /// itself.
    pub fn into_reqwest(self) -> ReqwestClient {
        self.raw
    }

    /// Downloads a file to `dest` using HTTP Range requests.
    ///
    /// `request_builder` is called once per chunk to produce a `RequestBuilder`
    /// for the given URL. The caller is responsible for adding any required
    /// headers (e.g. `Authorization`). Transient failures on `send()` (headers)
    /// are retried by the client middleware, which reuses that built request.
    ///
    /// # Known limitation (mid-body failure)
    ///
    /// Chunks are sized to use most of [`CLIENT_TIMEOUT_SECS`]. Middleware retries
    /// only wrap `send()` — once headers arrive, `.bytes().await` sits outside
    /// the retry policy. A Wi-Fi drop mid-body fails the whole download. A future
    /// streaming download (or an explicit send+body retry loop) should replace
    /// this; TODO: streaming/chunk-body-aware retry.
    ///
    /// `progress_callback` is called after each successful chunk with
    /// `(bytes_downloaded_so_far, total_bytes)`.
    ///
    /// # Errors
    ///
    /// Writes into a sibling staging file and atomically renames onto `dest` only
    /// after every chunk succeeds, so an existing artifact is preserved until the
    /// download completes. Cancellation, chunk failures, and write errors remove
    /// only the staging file. Publishing `dest` does not commit a [`CancelFlag`];
    /// callers with a later point of no return must call
    /// [`CancelFunc::try_commit`] themselves.
    ///
    /// Returns `ChunkedDownloadError::Io` if the staging file cannot be created,
    /// written, or renamed onto `dest`. Returns `ChunkedDownloadError::Request` if
    /// all retry attempts for any chunk fail. Returns
    /// `ChunkedDownloadError::Cancelled` when `should_cancel` reports cancellation
    /// between chunks or before the file is published.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cadmus_core::http::Client;
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
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
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, request_builder, progress_callback))
    )]
    pub async fn download<B, F>(
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

        let staging = download_staging_path(dest);
        tracing::debug!(path = ?staging, "Download staging");
        let mut unpublished = crate::fs::RemovePathOnDrop::file(staging.clone());
        let mut file = tokio::fs::File::create(&staging).await?;

        let mut downloaded = 0u64;
        let mut chunk_size = INITIAL_CHUNK_SIZE;

        tracing::debug!(
            initial_chunk_size = INITIAL_CHUNK_SIZE,
            "Starting chunked download"
        );

        while downloaded < total_size {
            if should_cancel.is_some_and(CancelFunc::is_cancelled) {
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
            let chunk_data =
                Self::download_chunk(url, chunk_start, chunk_end, &request_builder).await?;
            let elapsed_secs = start.elapsed().as_secs_f64();

            file.write_all(&chunk_data).await?;
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

        file.sync_all().await?;
        if should_cancel.is_some_and(CancelFunc::is_cancelled) {
            return Err(ChunkedDownloadError::Cancelled);
        }
        tokio::fs::rename(&staging, dest).await?;
        unpublished.disarm();

        tracing::debug!(bytes = downloaded, "Download complete");
        tracing::debug!(path = ?dest, "Saved file");

        Ok(())
    }

    /// Downloads a specific byte range from a URL using the HTTP `Range` header.
    ///
    /// Transient failures on `send()` / status are retried by the client
    /// middleware. Body read failures are not (see [`Self::download`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server returns a non-2xx status.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(request_builder)))]
    async fn download_chunk<B>(
        url: &str,
        start: u64,
        end: u64,
        request_builder: &B,
    ) -> Result<Vec<u8>, ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
    {
        let range_header = format!("bytes={start}-{end}");
        let bytes = request_builder(url)
            .header("Range", range_header)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        Ok(bytes.to_vec())
    }
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            client: self.client.clone(),
        }
    }
}

pub(crate) fn reqwest_error(error: reqwest_middleware::Error) -> Result<reqwest::Error, String> {
    match error {
        reqwest_middleware::Error::Reqwest(error) => Ok(error),
        reqwest_middleware::Error::Middleware(error) => {
            match error.downcast::<reqwest_retry::RetryError>() {
                Ok(retry) => match retry {
                    reqwest_retry::RetryError::Error(error) => reqwest_error(error),
                    reqwest_retry::RetryError::WithRetries { err, .. } => reqwest_error(err),
                },
                Err(error) => Err(error.to_string()),
            }
        }
    }
}

fn build_root_store() -> RootCertStore {
    let mut store = RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    store
}

fn download_staging_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    dest.with_file_name(format!("{name}.{}.partial", uuid::Uuid::now_v7()))
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

    #[tokio::test]
    async fn download_returns_cancelled_before_first_chunk() {
        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-cancel-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("partial.bin");
        std::fs::write(&dest, b"existing").expect("seed dest");

        let cancel_check = || true;
        let result = client
            .download(
                "https://example.invalid/unused",
                1024,
                &dest,
                |url| client.get(url),
                &mut |_, _| {},
                Some(CancelFunc::new(&cancel_check)),
            )
            .await;

        assert!(matches!(result, Err(ChunkedDownloadError::Cancelled)));
        assert_eq!(
            std::fs::read(&dest).expect("dest preserved"),
            b"existing",
            "failed download must not truncate an existing destination"
        );
        assert!(
            leftover_partials(temp_dir.path()).is_empty(),
            "staging partial must be cleaned up"
        );
    }

    #[tokio::test]
    async fn download_returns_cancelled_before_publish() {
        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-cancel-publish-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("artifact.bin");
        std::fs::write(&dest, b"existing").expect("seed dest");
        let flag = CancelFlag::new();
        flag.request_cancel();

        let result = client
            .download(
                "https://example.invalid/unused",
                0,
                &dest,
                |url| client.get(url),
                &mut |_, _| {},
                Some(CancelFunc::from_flag(&flag)),
            )
            .await;

        assert!(matches!(result, Err(ChunkedDownloadError::Cancelled)));
        assert_eq!(
            std::fs::read(&dest).expect("dest preserved"),
            b"existing",
            "cancel before publish must not replace an existing destination"
        );
        assert!(
            leftover_partials(temp_dir.path()).is_empty(),
            "staging partial must be cleaned up"
        );
    }

    #[tokio::test]
    async fn download_publish_leaves_cancel_flag_uncommitted() {
        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-no-commit-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("artifact.bin");
        let flag = CancelFlag::new();

        let result = client
            .download(
                "https://example.invalid/unused",
                0,
                &dest,
                |url| client.get(url),
                &mut |_, _| {},
                Some(CancelFunc::from_flag(&flag)),
            )
            .await;

        assert!(result.is_ok(), "empty download should publish dest");
        assert!(dest.exists(), "dest should be published");
        flag.request_cancel();
        assert!(
            flag.is_cancelled(),
            "zip publish must not lock out later cancel"
        );
    }

    fn leftover_partials(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("read_dir")
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                name.ends_with(".partial").then_some(name)
            })
            .collect()
    }
}
