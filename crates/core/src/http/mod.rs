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
//! - Optional per-request cancellation during those retries via a
//!   [`CancelFlag`] passed to [`Client::download`], surfaced to the retry
//!   middleware as a [`cancel::RequestCancel`]
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

mod cancel;
mod retry;

use reqwest::Client as ReqwestClient;
use reqwest_middleware::{ClientWithMiddleware, RequestBuilder};
use rustls::RootCertStore;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

use cancel::RequestCancel;
pub use cancel::{CancelFlag, CancelFunc};
use retry::client_with_retry;

pub const CLIENT_TIMEOUT_SECS: u64 = 30;
pub const CLIENT_CONNECT_TIMEOUT_SECS: u64 = 10;

pub(crate) const USER_AGENT: &str = concat!("github.com/OGKevin/cadmus/", env!("GIT_VERSION"));

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
const RETRY_BACKOFFS: usize = 2;
/// Re-requests of the same range after the body is interrupted. A drop costs the
/// bytes in flight, not the chunk, so this only needs to absorb a flaky link
/// rather than survive an outage.
const MAX_BODY_RESUMES: usize = 3;

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
        if let reqwest_middleware::Error::Middleware(ref inner) = error
            && inner.is::<retry::RequestCancelled>()
        {
            return Self::Cancelled;
        }
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
    /// between chunks, during retry backoff, or before the file is published.
    ///
    /// Cancellation uses [`CancelFlag`] so the check can live in request
    /// extensions. A borrowed [`CancelFunc::Check`] cannot; pass a flag (or
    /// `None`) at this call site.
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
        dest: &Path,
        request_builder: B,
        progress_callback: &mut F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<(), ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
        F: FnMut(u64, u64),
    {
        let mut throttled_progress = TenthsProgress::new(progress_callback);
        throttled_progress.report(0, total_size);

        tracing::debug!(url = %url, "Downloading file");
        tracing::debug!(path = ?dest, "Download destination");

        let staging = download_staging_path(dest);
        tracing::debug!(path = ?staging, "Download staging");
        let mut unpublished = crate::fs::RemovePathOnDrop::file(staging.clone());
        let result: Result<(), ChunkedDownloadError> = async {
            let mut file = tokio::fs::File::create(&staging).await?;

            let mut downloaded = 0u64;
            let mut chunk_size = INITIAL_CHUNK_SIZE;

            tracing::debug!(
                initial_chunk_size = INITIAL_CHUNK_SIZE,
                "Starting chunked download"
            );

            while downloaded < total_size {
                if should_cancel.is_some_and(CancelFlag::is_cancelled) {
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
                let before = downloaded;
                self.stream_chunk(
                    &mut file,
                    &mut downloaded,
                    chunk_end,
                    total_size,
                    url,
                    &request_builder,
                    should_cancel,
                    |done, total| throttled_progress.report(done, total),
                )
                .await?;
                let written = downloaded - before;
                let elapsed_secs = start.elapsed().as_secs_f64();

                if elapsed_secs > 0.0 {
                    let throughput = written as f64 / elapsed_secs;
                    chunk_size = ((throughput * TARGET_CHUNK_SECS) as usize)
                        .clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE);
                    tracing::debug!(
                        elapsed_secs,
                        throughput_bytes_per_sec = throughput as u64,
                        next_chunk_size = chunk_size,
                        "Adjusted chunk size"
                    );
                }

                tracing::debug!(
                    downloaded,
                    total_size,
                    progress_percent = (downloaded as f64 / total_size as f64) * 100.0,
                    "Download progress"
                );
            }

            file.sync_all().await?;
            if should_cancel.is_some_and(CancelFlag::is_cancelled) {
                return Err(ChunkedDownloadError::Cancelled);
            }
            tokio::fs::rename(&staging, dest).await?;

            tracing::debug!(bytes = downloaded, "Download complete");
            tracing::debug!(path = ?dest, "Saved file");

            Ok(())
        }
        .await;

        match result {
            Ok(()) => {
                unpublished.disarm();
                Ok(())
            }
            Err(error) => {
                unpublished.remove_if_armed().await;
                Err(error)
            }
        }
    }

    /// Streams `[downloaded, chunk_end]` straight into `file`, advancing
    /// `downloaded` as bytes land.
    ///
    /// The body is never buffered, so peak RAM is one network chunk rather than
    /// one range chunk, and progress is reported per read through
    /// `progress_callback`.
    ///
    /// A Wi-Fi drop mid-body is resumed rather than restarted: the range is
    /// re-requested from `*downloaded`, so a drop costs the bytes in flight and
    /// not the whole chunk. `send()` failures are already covered by the retry
    /// middleware; this covers what happens after the headers arrive.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, request_builder, progress_callback, file), fields(start = %downloaded, end = chunk_end)))]
    #[allow(clippy::too_many_arguments)]
    async fn stream_chunk<B, P>(
        &self,
        file: &mut tokio::fs::File,
        downloaded: &mut u64,
        chunk_end: u64,
        total_size: u64,
        url: &str,
        request_builder: &B,
        should_cancel: Option<&CancelFlag>,
        mut progress_callback: P,
    ) -> Result<(), ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
        P: FnMut(u64, u64),
    {
        use tokio::io::AsyncWriteExt as _;

        let mut resumes = 0usize;
        loop {
            if *downloaded > chunk_end {
                return Ok(());
            }
            {
                if should_cancel.is_some_and(CancelFlag::is_cancelled) {
                    return Err(ChunkedDownloadError::Cancelled);
                }

                let range_header = format!("bytes={}-{chunk_end}", *downloaded);
                let mut request = request_builder(url).header("Range", range_header);
                if let Some(flag) = should_cancel {
                    request = request.with_extension(RequestCancel::from_flag(flag));
                }
                let response = request.send().await?.error_for_status()?;
                if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                    return Err(ChunkedDownloadError::Failed(format!(
                        "range request expected 206 Partial Content, got {}",
                        response.status()
                    )));
                }

                if let Some(header) = response.headers().get(http::header::CONTENT_RANGE) {
                    let header = header.to_str().map_err(|_| {
                        ChunkedDownloadError::Failed("invalid Content-Range header".to_owned())
                    })?;
                    validate_content_range(header, *downloaded, chunk_end)?;
                }

                let mut response = response;
                let mut body_started = false;
                let mut interrupted = None;
                loop {
                    match response.chunk().await {
                        Ok(Some(bytes)) => {
                            body_started = true;
                            file.write_all(&bytes).await?;
                            *downloaded += bytes.len() as u64;
                            progress_callback(*downloaded, total_size);
                        }
                        Ok(None) => break,
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                downloaded = *downloaded,
                                "chunk body interrupted; resuming from the last byte written"
                            );
                            interrupted = Some(error);
                            break;
                        }
                    }
                }

                if interrupted.is_some() {
                    resumes += 1;
                    if resumes > MAX_BODY_RESUMES {
                        return Err(ChunkedDownloadError::Failed(format!(
                            "chunk body interrupted more than {MAX_BODY_RESUMES} times"
                        )));
                    }
                    continue;
                }

                if !body_started && *downloaded <= chunk_end {
                    return Err(ChunkedDownloadError::Failed(
                        "range response body was empty".to_owned(),
                    ));
                }
            }
        }
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

/// Checks that a `Content-Range` value matches the requested byte span.
fn validate_content_range(
    header: &str,
    expected_start: u64,
    expected_end: u64,
) -> Result<(), ChunkedDownloadError> {
    let (start, end) = parse_content_range_span(header).ok_or_else(|| {
        ChunkedDownloadError::Failed(format!("unparseable Content-Range: {header}"))
    })?;
    if start != expected_start || end != expected_end {
        return Err(ChunkedDownloadError::Failed(format!(
            "Content-Range {start}-{end} does not match requested bytes={expected_start}-{expected_end}"
        )));
    }
    Ok(())
}

/// Parses `bytes start-end/total` from a `Content-Range` header value.
fn parse_content_range_span(header: &str) -> Option<(u64, u64)> {
    let bytes = header.strip_prefix("bytes ")?;
    let (range, _) = bytes.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    Some((start, end))
}

fn build_root_store() -> RootCertStore {
    let mut store = RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    store
}

/// Emits progress at 0%, each 10% step, and 100%.
struct TenthsProgress<'a, F> {
    inner: &'a mut F,
    last_step: Option<u8>,
}

impl<'a, F: FnMut(u64, u64)> TenthsProgress<'a, F> {
    fn new(inner: &'a mut F) -> Self {
        Self {
            inner,
            last_step: None,
        }
    }

    fn report(&mut self, downloaded: u64, total: u64) {
        if total == 0 {
            (self.inner)(downloaded, total);
            return;
        }
        let percent = downloaded.saturating_mul(100).div_ceil(total).min(100) as u8;
        let step = if downloaded >= total {
            10
        } else {
            percent / 10
        };
        let should_emit = downloaded == 0 || downloaded >= total || self.last_step != Some(step);
        if should_emit {
            self.last_step = Some(step);
            (self.inner)(downloaded, total);
        }
    }
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
    fn parse_content_range_span_reads_byte_range() {
        assert_eq!(parse_content_range_span("bytes 0-0/1024"), Some((0, 0)));
        assert_eq!(
            parse_content_range_span("bytes 100-255/1024"),
            Some((100, 255))
        );
        assert!(parse_content_range_span("bytes */1024").is_none());
    }

    #[test]
    fn validate_content_range_rejects_mismatched_span() {
        let err = validate_content_range("bytes 0-99/1024", 100, 255).expect_err("mismatch");
        assert!(err.to_string().contains("does not match"));
    }

    #[test]
    fn tenths_progress_emits_start_step_and_completion() {
        let mut reports = Vec::new();
        let mut reporter = |downloaded: u64, total: u64| {
            reports.push((downloaded, total));
        };
        let mut progress = TenthsProgress::new(&mut reporter);
        let total = 1000u64;
        progress.report(0, total);
        progress.report(50, total);
        progress.report(150, total);
        progress.report(250, total);
        progress.report(1000, total);
        assert_eq!(
            reports,
            vec![(0, total), (150, total), (250, total), (1000, total)]
        );
    }

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

    /// A range server that deliberately truncates every body.
    ///
    /// Each response advertises the full remaining range in `Content-Length`
    /// but sends only [`TRUNCATED_BODY_BYTES`], then closes. A client that
    /// restarts the whole chunk on a short read would loop forever; a client
    /// that resumes from the last byte written finishes. Returns the port and
    /// a counter of requests served.
    async fn serve_truncating_ranges(
        total: u64,
    ) -> (
        u16,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        const TRUNCATED_BODY_BYTES: usize = 4096;

        let served = std::sync::Arc::new(AtomicUsize::new(0));
        let served_by_task = std::sync::Arc::clone(&served);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();

        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                if stream.read(&mut buf).await.is_err() {
                    continue;
                }
                let request = String::from_utf8_lossy(&buf);
                // Answer with what is left of the file from the requested offset.
                let start: u64 = request
                    .split("bytes=")
                    .nth(1)
                    .and_then(|rest| rest.split('-').next())
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                let remaining = total.saturating_sub(start);
                if remaining == 0 {
                    break;
                }
                let advertised = remaining.min(TRUNCATED_BODY_BYTES as u64);
                let header =
                    format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {advertised}\r\n\r\n");
                let _ = stream.write_all(header.as_bytes()).await;
                // Advertised length equals what we send, so the client sees a
                // *short* range rather than a truncated stream; the resume path
                // is what carries it forward.
                let body = vec![b'A'; advertised as usize];
                let _ = stream.write_all(&body).await;
                let _ = stream.flush().await;
                served_by_task.fetch_add(1, Ordering::SeqCst);
            }
        });

        (port, served, handle)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chunked_download_resumes_after_a_short_body() {
        use std::sync::atomic::Ordering;

        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-resume-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("artifact.bin");

        let total = 64 * 1024u64;
        let (port, served, server) = serve_truncating_ranges(total).await;
        let url = format!("http://127.0.0.1:{port}/artifact.bin");

        let result = client
            .download(
                &url,
                total,
                &dest,
                |url| client.get(url),
                &mut |_, _| {},
                None,
            )
            .await;

        server.abort();
        assert!(result.is_ok(), "resume should complete: {result:?}");
        assert!(
            served.load(Ordering::SeqCst) > 1,
            "a short body must be re-requested, not served once and abandoned"
        );
        assert_eq!(
            std::fs::metadata(&dest).expect("dest").len(),
            total,
            "resumed download must still produce the full length"
        );
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

        let flag = CancelFlag::new();
        flag.request_cancel();
        let result = client
            .download(
                "https://example.invalid/unused",
                1024,
                &dest,
                |url| client.get(url),
                &mut |_, _| {},
                Some(&flag),
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
                Some(&flag),
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
                Some(&flag),
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
