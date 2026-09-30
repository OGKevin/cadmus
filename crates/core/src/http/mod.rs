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
//! Middleware retries wrap `send()` (headers). Mid-body read failures are
//! recovered by [`Client::download`], which re-requests the range.
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
    /// The download tries one `bytes=0-(total_size - 1)` range first, then
    /// adaptive smaller ranges if that fails or stops early. `request_builder`
    /// is called once per range request to produce a `RequestBuilder`
    /// for the given URL. The caller is responsible for adding any required
    /// headers (e.g. `Authorization`). Transient failures on `send()` (headers)
    /// are retried by the client middleware, which reuses that built request.
    ///
    /// # Mid-body failures
    ///
    /// Middleware retries only wrap `send()` — once headers arrive, the body is
    /// streamed outside the retry policy. A Wi-Fi drop mid-body is recovered by
    /// re-requesting the range from the last byte written, so the drop costs the
    /// bytes in flight rather than the whole chunk. A chunk whose body keeps
    /// being interrupted fails after a bounded number of retries.
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
    ///     Some(1024 * 1024),
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
        total_size: Option<u64>,
        dest: &Path,
        request_builder: B,
        progress_callback: &mut F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<(), ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
        F: FnMut(u64, u64),
    {
        tracing::debug!(url = %url, "Downloading file");
        tracing::debug!(path = ?dest, "Download destination");

        let staging = download_staging_path(dest);
        tracing::debug!(path = ?staging, "Download staging");
        let mut unpublished = crate::fs::RemovePathOnDrop::file(staging.clone());
        let result: Result<(), ChunkedDownloadError> = async {
            let mut file = tokio::fs::File::create(&staging).await?;
            self.download_into(
                &mut file,
                total_size,
                url,
                &request_builder,
                progress_callback,
                should_cancel,
            )
            .await?;

            file.sync_all().await?;
            if should_cancel.is_some_and(CancelFlag::is_cancelled) {
                return Err(ChunkedDownloadError::Cancelled);
            }
            tokio::fs::rename(&staging, dest).await?;

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

    /// Downloads a file into memory using HTTP Range requests.
    ///
    /// Identical chunk sizing, retries, progress reporting, and cancellation to
    /// [`Client::download`], but received bytes are appended to a [`Vec`] instead
    /// of a staging file. Nothing touches disk, so there is no atomic publish;
    /// the caller owns the returned buffer.
    ///
    /// # Errors
    ///
    /// Returns `ChunkedDownloadError::Request` if all retry attempts for any
    /// chunk fail, and `ChunkedDownloadError::Cancelled` when `should_cancel`
    /// reports cancellation between chunks or during a chunk resume.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, request_builder, progress_callback))
    )]
    pub async fn download_to_vec<B, F>(
        &self,
        url: &str,
        total_size: Option<u64>,
        request_builder: B,
        progress_callback: &mut F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<Vec<u8>, ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
        F: FnMut(u64, u64),
    {
        tracing::debug!(url = %url, "Downloading file into memory");

        let capacity = total_size
            .and_then(|total| usize::try_from(total).ok())
            .unwrap_or(0);
        let mut bytes = Vec::with_capacity(capacity);
        self.download_into(
            &mut bytes,
            total_size,
            url,
            &request_builder,
            progress_callback,
            should_cancel,
        )
        .await?;

        tracing::debug!(bytes = bytes.len(), "In-memory download complete");
        Ok(bytes)
    }

    /// Resolves the download size with a HEAD request when the caller does not
    /// know it.
    ///
    /// Only used for unauthenticated URLs: this HEAD does not carry the caller's
    /// `request_builder`, so it cannot send an `Authorization` header. Pass the
    /// size explicitly (`Some`) for authenticated downloads.
    async fn resolve_total_size(&self, url: &str) -> Result<u64, ChunkedDownloadError> {
        let response = self.head(url).send().await?.error_for_status()?;
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .ok_or_else(|| {
                ChunkedDownloadError::Failed(
                    "HEAD response did not include a Content-Length".to_owned(),
                )
            })
    }

    /// Runs the download loop, writing every received byte into `sink`.
    ///
    /// Shared by [`Client::download`] (staging file) and
    /// [`Client::download_to_vec`] (in-memory buffer). The first attempt is a
    /// **single** whole-file range request `bytes=0-(total_size - 1)` that does
    /// not retry the same range on a mid-body interruption; if it fails or stops
    /// early, the remainder is fetched with adaptive chunk windows resuming from
    /// the last byte written. Progress is throttled to tenths by
    /// [`TenthsProgress`] on every body read, including during a whole-file
    /// request.
    async fn download_into<W, B, F>(
        &self,
        sink: &mut W,
        total_size: Option<u64>,
        url: &str,
        request_builder: &B,
        progress_callback: &mut F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<(), ChunkedDownloadError>
    where
        W: tokio::io::AsyncWrite + Unpin,
        B: Fn(&str) -> RequestBuilder,
        F: FnMut(u64, u64),
    {
        let total_size = match total_size {
            Some(total) => total,
            None => self.resolve_total_size(url).await?,
        };
        let mut throttled_progress = TenthsProgress::new(progress_callback);
        throttled_progress.report(0, total_size);

        let mut downloaded = 0u64;
        let mut chunk_size = INITIAL_CHUNK_SIZE;

        if total_size > 0 {
            let whole_end = total_size - 1;
            tracing::debug!(
                total_size,
                whole_end,
                "Attempting whole-file range download"
            );
            let before = downloaded;
            let whole_chunk = self.stream_chunk(
                sink,
                &mut downloaded,
                whole_end,
                total_size,
                url,
                request_builder,
                should_cancel,
                0,
                |done, total| throttled_progress.report(done, total),
            );
            #[cfg(feature = "tracing")]
            let whole_chunk = {
                use tracing::Instrument as _;
                whole_chunk.instrument(tracing::info_span!(
                    "whole_file_range",
                    total_size,
                    range_end = whole_end
                ))
            };
            match whole_chunk.await {
                Ok(()) if downloaded >= total_size => {
                    tracing::debug!(bytes = downloaded, "Whole-file range download complete");
                    return Ok(());
                }
                Ok(()) => {
                    tracing::debug!(
                        downloaded,
                        total_size,
                        "Whole-file range download stopped early; continuing with chunked download"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        downloaded,
                        total_size,
                        "Whole-file range download failed; falling back to chunked download"
                    );
                    if downloaded <= before {
                        tracing::trace!("Whole-file attempt made no progress");
                    }
                }
            }
        }

        tracing::debug!(
            initial_chunk_size = INITIAL_CHUNK_SIZE,
            downloaded,
            "Starting adaptive chunked download"
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
            let adaptive_chunk = self.stream_chunk(
                sink,
                &mut downloaded,
                chunk_end,
                total_size,
                url,
                request_builder,
                should_cancel,
                MAX_BODY_RESUMES,
                |done, total| throttled_progress.report(done, total),
            );
            #[cfg(feature = "tracing")]
            let adaptive_chunk = {
                use tracing::Instrument as _;
                adaptive_chunk.instrument(tracing::info_span!(
                    "adaptive_range",
                    chunk_start,
                    chunk_end,
                    chunk_size
                ))
            };
            adaptive_chunk.await?;
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

        tracing::debug!(bytes = downloaded, "Download complete");
        Ok(())
    }

    /// Streams `[downloaded, chunk_end]` straight into `sink`, advancing
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
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, request_builder, progress_callback, sink, should_cancel),
            fields(
                range_start = *downloaded,
                range_end = chunk_end,
                total_size,
                whole_file = chunk_end + 1 >= total_size && *downloaded == 0,
                url = %url,
                bytes_written = tracing::field::Empty,
                bytes_at_end = tracing::field::Empty,
                range_requests = tracing::field::Empty,
                body_resumes = tracing::field::Empty,
                duration_ms = tracing::field::Empty,
                throughput_bytes_per_sec = tracing::field::Empty,
                outcome = tracing::field::Empty,
            )
        )
    )]
    #[allow(clippy::too_many_arguments)]
    async fn stream_chunk<W, B, P>(
        &self,
        sink: &mut W,
        downloaded: &mut u64,
        chunk_end: u64,
        total_size: u64,
        url: &str,
        request_builder: &B,
        should_cancel: Option<&CancelFlag>,
        max_body_resumes: usize,
        mut progress_callback: P,
    ) -> Result<(), ChunkedDownloadError>
    where
        W: tokio::io::AsyncWrite + Unpin,
        B: Fn(&str) -> RequestBuilder,
        P: FnMut(u64, u64),
    {
        use tokio::io::AsyncWriteExt as _;

        #[cfg(feature = "tracing")]
        let range_start = *downloaded;
        #[cfg(feature = "tracing")]
        let started = std::time::Instant::now();
        #[cfg(feature = "tracing")]
        let mut range_requests = 0usize;
        let mut body_resumes = 0usize;
        loop {
            if *downloaded > chunk_end {
                #[cfg(feature = "tracing")]
                record_stream_chunk_span(
                    range_start,
                    *downloaded,
                    started,
                    range_requests,
                    body_resumes,
                    "already_complete",
                );
                return Ok(());
            }
            #[cfg(feature = "tracing")]
            {
                range_requests += 1;
            }
            #[cfg(feature = "tracing")]
            let attempt_index = range_requests;
            #[cfg(feature = "tracing")]
            let range_start_byte = *downloaded;
            #[cfg(feature = "tracing")]
            let after_body_resume = body_resumes > 0;

            enum RangeAttemptFlow {
                ResumeAfterInterrupt,
                ChunkComplete,
            }

            let attempt = async {
                if should_cancel.is_some_and(CancelFlag::is_cancelled) {
                    #[cfg(feature = "tracing")]
                    record_span_fields(0, 0, "cancelled");
                    return Err(ChunkedDownloadError::Cancelled);
                }

                let range_header = format!("bytes={}-{chunk_end}", *downloaded);
                let mut request = request_builder(url).header("Range", range_header);
                if let Some(flag) = should_cancel {
                    request = request.with_extension(RequestCancel::from_flag(flag));
                }
                let response = match request.send().await?.error_for_status() {
                    Ok(response) => response,
                    Err(error) => {
                        #[cfg(feature = "tracing")]
                        record_span_fields(0, 0, "request_failed");
                        return Err(ChunkedDownloadError::Request(error));
                    }
                };
                if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                    #[cfg(feature = "tracing")]
                    record_span_fields(0, 0, "unexpected_status");
                    return Err(ChunkedDownloadError::Failed(format!(
                        "range request expected 206 Partial Content, got {}",
                        response.status()
                    )));
                }

                if let Some(header) = response.headers().get(http::header::CONTENT_RANGE) {
                    let header = header.to_str().map_err(|_| {
                        ChunkedDownloadError::Failed("invalid Content-Range header".to_owned())
                    })?;
                    let content_range_ok = validate_content_range(header, *downloaded, chunk_end);
                    #[cfg(feature = "tracing")]
                    let content_range_ok = content_range_ok.inspect_err(|_| {
                        record_span_fields(0, 0, "content_range_mismatch");
                    });
                    content_range_ok?;
                }

                let mut response = response;
                #[cfg(feature = "tracing")]
                let bytes_before_body = *downloaded;
                let mut chunk_reads = 0u64;
                let read_body = async {
                    let mut body_started = false;
                    let mut interrupted = None;
                    loop {
                        match response.chunk().await {
                            Ok(Some(bytes)) => {
                                body_started = true;
                                chunk_reads += 1;
                                sink.write_all(&bytes).await?;
                                *downloaded += bytes.len() as u64;
                                progress_callback(*downloaded, total_size);
                            }
                            Ok(None) => break,
                            Err(error) => {
                                if body_resumes < max_body_resumes {
                                    tracing::warn!(
                                        error = %error,
                                        downloaded = *downloaded,
                                        "chunk body interrupted; resuming from the last byte written"
                                    );
                                } else {
                                    tracing::debug!(
                                        error = %error,
                                        downloaded = *downloaded,
                                        "chunk body interrupted; not resuming this range"
                                    );
                                }
                                interrupted = Some(error);
                                break;
                            }
                        }
                    }
                    #[cfg(feature = "tracing")]
                    record_span_fields(
                        downloaded.saturating_sub(bytes_before_body),
                        chunk_reads,
                        if interrupted.is_some() {
                            "interrupted"
                        } else {
                            "complete"
                        },
                    );
                    Ok::<_, ChunkedDownloadError>((body_started, interrupted, chunk_reads))
                };

                #[cfg(feature = "tracing")]
                let (body_started, interrupted, chunk_reads) = {
                    use tracing::Instrument as _;
                    read_body
                        .instrument(tracing::info_span!(
                            "read_response_body",
                            body_start = bytes_before_body,
                            bytes_read = tracing::field::Empty,
                            chunk_reads = tracing::field::Empty,
                            outcome = tracing::field::Empty,
                        ))
                        .await?
                };
                #[cfg(not(feature = "tracing"))]
                let (body_started, interrupted, _chunk_reads) = read_body.await?;

                #[cfg(feature = "tracing")]
                let bytes_read = downloaded.saturating_sub(bytes_before_body);

                if interrupted.is_some() {
                    #[cfg(feature = "tracing")]
                    record_span_fields(bytes_read, chunk_reads, "body_interrupted");
                    return Ok(RangeAttemptFlow::ResumeAfterInterrupt);
                }

                if !body_started && *downloaded <= chunk_end {
                    #[cfg(feature = "tracing")]
                    record_span_fields(0, 0, "empty_body");
                    return Err(ChunkedDownloadError::Failed(
                        "range response body was empty".to_owned(),
                    ));
                }

                #[cfg(feature = "tracing")]
                record_span_fields(bytes_read, chunk_reads, "complete");
                Ok(RangeAttemptFlow::ChunkComplete)
            };

            #[cfg(feature = "tracing")]
            let attempt = {
                use tracing::Instrument as _;
                attempt.instrument(tracing::info_span!(
                    "range_attempt",
                    attempt = attempt_index,
                    range_start = range_start_byte,
                    range_end = chunk_end,
                    after_body_resume = after_body_resume,
                    bytes_read = tracing::field::Empty,
                    chunk_reads = tracing::field::Empty,
                    outcome = tracing::field::Empty,
                ))
            };

            match attempt.await {
                Ok(RangeAttemptFlow::ResumeAfterInterrupt) => {
                    body_resumes += 1;
                    if body_resumes > max_body_resumes {
                        #[cfg(feature = "tracing")]
                        record_stream_chunk_span(
                            range_start,
                            *downloaded,
                            started,
                            range_requests,
                            body_resumes,
                            "body_resume_exhausted",
                        );
                        return Err(ChunkedDownloadError::Failed(format!(
                            "chunk body interrupted more than {max_body_resumes} times"
                        )));
                    }
                    continue;
                }
                Ok(RangeAttemptFlow::ChunkComplete) => {
                    if *downloaded > chunk_end {
                        #[cfg(feature = "tracing")]
                        record_stream_chunk_span(
                            range_start,
                            *downloaded,
                            started,
                            range_requests,
                            body_resumes,
                            "complete",
                        );
                        return Ok(());
                    }
                }
                Err(ChunkedDownloadError::Cancelled) => {
                    #[cfg(feature = "tracing")]
                    record_stream_chunk_span(
                        range_start,
                        *downloaded,
                        started,
                        range_requests,
                        body_resumes,
                        "cancelled",
                    );
                    return Err(ChunkedDownloadError::Cancelled);
                }
                Err(error) => {
                    #[cfg(feature = "tracing")]
                    record_stream_chunk_span(
                        range_start,
                        *downloaded,
                        started,
                        range_requests,
                        body_resumes,
                        "failed",
                    );
                    return Err(error);
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

#[cfg(feature = "tracing")]
fn record_span_fields(bytes_read: u64, chunk_reads: u64, outcome: &'static str) {
    let span = tracing::Span::current();
    span.record("bytes_read", bytes_read);
    span.record("chunk_reads", chunk_reads);
    span.record("outcome", outcome);
}

#[cfg(feature = "tracing")]
fn record_stream_chunk_span(
    range_start: u64,
    downloaded: u64,
    started: std::time::Instant,
    range_requests: usize,
    body_resumes: usize,
    outcome: &'static str,
) {
    let bytes_written = downloaded.saturating_sub(range_start);
    let elapsed = started.elapsed();
    let duration_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
    let throughput_bytes_per_sec = if elapsed.as_secs_f64() > 0.0 {
        (bytes_written as f64 / elapsed.as_secs_f64()) as u64
    } else {
        0
    };
    let span = tracing::Span::current();
    span.record("bytes_written", bytes_written);
    span.record("bytes_at_end", downloaded);
    span.record("range_requests", range_requests as u64);
    span.record("body_resumes", body_resumes as u64);
    span.record("duration_ms", duration_ms);
    span.record("throughput_bytes_per_sec", throughput_bytes_per_sec);
    span.record("outcome", outcome);
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
                Some(total),
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
                Some(1024),
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chunked_download_to_vec_resumes_after_a_short_body() {
        use std::sync::atomic::Ordering;

        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");

        let total = 64 * 1024u64;
        let (port, served, server) = serve_truncating_ranges(total).await;
        let url = format!("http://127.0.0.1:{port}/artifact.bin");

        let bytes = client
            .download_to_vec(
                &url,
                Some(total),
                |url| client.get(url),
                &mut |_, _| {},
                None,
            )
            .await
            .expect("in-memory download should complete");

        server.abort();
        assert_eq!(bytes.len() as u64, total);
        assert!(bytes.iter().all(|&byte| byte == b'A'));
        assert!(
            served.load(Ordering::SeqCst) > 1,
            "a short body must be re-requested, not served once and abandoned"
        );
    }

    #[tokio::test]
    async fn download_to_vec_returns_cancelled_before_first_chunk() {
        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let flag = CancelFlag::new();
        flag.request_cancel();

        let result = client
            .download_to_vec(
                "https://example.invalid/unused",
                Some(1024),
                |url| client.get(url),
                &mut |_, _| {},
                Some(&flag),
            )
            .await;

        assert!(matches!(result, Err(ChunkedDownloadError::Cancelled)));
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
                Some(0),
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
                Some(0),
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

    #[cfg(feature = "tracing")]
    #[test]
    fn download_spans_record_declared_fields() {
        use std::sync::{Arc, Mutex};

        struct SharedWriter(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for SharedWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-span-fields-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("artifact.bin");

        let total = 64 * 1024u64;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime");
        let (port, _served, server) = runtime.block_on(serve_truncating_ranges(total));
        let url = format!("http://127.0.0.1:{port}/artifact.bin");

        let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = {
            let buffer = Arc::clone(&buffer);
            move || SharedWriter(Arc::clone(&buffer))
        };
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_ansi(false)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_max_level(tracing::Level::TRACE)
            .finish();

        // `DefaultGuard` is not allowed across an await point, so drive the
        // download through a synchronous `block_on` while it is set.
        let guard = tracing::subscriber::set_default(subscriber);
        let result = runtime.block_on(async {
            client
                .download(
                    &url,
                    Some(total),
                    &dest,
                    |url| client.get(url),
                    &mut |_, _| {},
                    None,
                )
                .await
        });
        drop(guard);
        server.abort();
        assert!(result.is_ok(), "download should complete: {result:?}");

        let output = String::from_utf8(buffer.lock().unwrap().clone()).expect("utf8");
        assert!(
            output.contains("stream_chunk"),
            "stream_chunk span missing from captured output:\n{output}"
        );
        assert!(
            output.contains("outcome="),
            "recorded outcome field missing (span record is a no-op?):\n{output}"
        );
        assert!(
            output.contains("bytes_written="),
            "recorded bytes_written field missing (span record is a no-op?):\n{output}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn download_resolves_missing_total_size_with_head() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-head-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("artifact.bin");

        let total = 8 * 1024u64;
        let saw_head = Arc::new(AtomicBool::new(false));
        let saw_head_srv = Arc::clone(&saw_head);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                if stream.read(&mut buf).await.is_err() {
                    continue;
                }
                let request = String::from_utf8_lossy(&buf).to_string();
                if request.starts_with("HEAD ") {
                    saw_head_srv.store(true, Ordering::SeqCst);
                    let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\r\n");
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.flush().await;
                    continue;
                }
                let (start, end) = request
                    .split("bytes=")
                    .nth(1)
                    .and_then(|rest| {
                        let mut parts = rest.split('-');
                        let start: u64 = parts.next()?.trim().parse().ok()?;
                        let end: u64 = parts.next()?.split_whitespace().next()?.parse().ok()?;
                        Some((start, end))
                    })
                    .unwrap_or((0, total - 1));
                let len = end - start + 1;
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {len}\r\nContent-Range: bytes {start}-{end}/{total}\r\n\r\n"
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(&vec![b'C'; len as usize]).await;
                let _ = stream.flush().await;
            }
        });

        let url = format!("http://127.0.0.1:{port}/artifact.bin");
        let result = client
            .download(
                &url,
                None,
                &dest,
                |url| client.get(url),
                &mut |_, _| {},
                None,
            )
            .await;
        server.abort();

        assert!(result.is_ok(), "download should complete: {result:?}");
        assert!(
            saw_head.load(Ordering::SeqCst),
            "a missing total_size must trigger a HEAD"
        );
        assert_eq!(
            std::fs::metadata(&dest).expect("dest").len(),
            total,
            "HEAD-resolved download must produce the full length"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn whole_file_attempt_is_single_request_then_falls_back_to_chunks() {
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        crate::crypto::init_crypto_provider();
        let client = Client::new().expect("client");
        let temp_dir = tempfile::Builder::new()
            .prefix("cadmus-http-whole-file-")
            .tempdir()
            .expect("tempdir");
        let dest = temp_dir.path().join("artifact.bin");

        let total = 3 * 1024 * 1024u64;
        let requests: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let requests_srv = Arc::clone(&requests);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(async move {
            let mut first = true;
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                if stream.read(&mut buf).await.is_err() {
                    continue;
                }
                let request = String::from_utf8_lossy(&buf).to_string();
                let range = request.split("bytes=").nth(1).and_then(|rest| {
                    let mut parts = rest.split('-');
                    let start: u64 = parts.next()?.trim().parse().ok()?;
                    let end: u64 = parts.next()?.split_whitespace().next()?.parse().ok()?;
                    Some((start, end))
                });
                if let Some(span) = range {
                    requests_srv.lock().unwrap().push(span);
                }
                if first {
                    first = false;
                    let header =
                        format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {total}\r\n\r\n");
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&vec![b'A'; 4096]).await;
                    let _ = stream.flush().await;
                    continue;
                }
                if let Some((start, end)) = range {
                    let len = end - start + 1;
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {len}\r\nContent-Range: bytes {start}-{end}/{total}\r\n\r\n"
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&vec![b'B'; len as usize]).await;
                    let _ = stream.flush().await;
                }
            }
        });

        let url = format!("http://127.0.0.1:{port}/artifact.bin");
        let result = client
            .download(
                &url,
                Some(total),
                &dest,
                |url| client.get(url),
                &mut |_, _| {},
                None,
            )
            .await;
        server.abort();

        assert!(result.is_ok(), "download should recover: {result:?}");
        let requests = requests.lock().unwrap();
        assert!(
            requests.len() > 1,
            "an interrupted whole-file attempt must be followed by chunked requests: {requests:?}"
        );
        assert_eq!(
            requests[0],
            (0, total - 1),
            "the first request must be the whole-file range"
        );
        assert_eq!(
            std::fs::metadata(&dest).expect("dest").len(),
            total,
            "the resumed download must produce the full length"
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
