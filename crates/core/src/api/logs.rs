//! Companion-only Logs service: the `cadmus.api.public.v1.Logs` contract.
//!
//! [`PublicRpc`] in [`super::public_rpc`] owns the request state; this module
//! holds the log formats, the scans, and the service implementation.

mod bundle;
mod catalog;
mod download;
mod filter;
mod page;
mod summary;

pub(super) use catalog::LogSources;
pub(super) use summary::SummaryCache;

use super::public_rpc::PublicRpc;
use bundle::BundleFile;
use cadmus_api_proto::cadmus::api::public::v1::{
    DownloadEvent, DownloadLogsRequest, ListLogsRequest, LogLine, LogList, LogPage, LogSummary,
    Logs, ReadDirection, ReadLogRequest, SummarizeLogRequest,
};
use connectrpc::{
    ConnectError, Encodable, RequestContext, ServiceRequest, ServiceResult, ServiceStream,
};
use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::sync::Arc;
use summary::{Plan, Stamp, Tally};
use tokio::sync::mpsc;

/// Snapshots buffered between a scan and its response stream.
const SNAPSHOT_BACKLOG: usize = 2;

fn log_not_found() -> ConnectError {
    ConnectError::not_found("log not found")
}

fn api_stopped() -> ConnectError {
    ConnectError::unavailable("api stopped")
}

/// The Connect error for a failed log read.
///
/// A deleted file and a bad cursor are the caller's to fix. Anything else is
/// logged here and reported without the path.
fn io_error(error: io::Error) -> ConnectError {
    match error.kind() {
        io::ErrorKind::NotFound => log_not_found(),
        io::ErrorKind::InvalidInput => ConnectError::invalid_argument(error.to_string()),
        _ => {
            tracing::warn!(error = %error, "log read failed");
            ConnectError::internal("log read failed")
        }
    }
}

fn page_request(request: &ReadLogRequest) -> page::Request {
    let anchor = if request.from_end {
        page::Anchor::End
    } else {
        page::Anchor::Cursor(request.cursor)
    };
    let direction = match request.direction.as_known() {
        Some(ReadDirection::READ_DIRECTION_FORWARD) => page::Direction::Forward,
        _ => page::Direction::Backward,
    };
    let limit = match request.limit {
        0 => page::DEFAULT_LIMIT,
        limit => limit as usize,
    };
    page::Request {
        anchor,
        direction,
        limit,
        filter: filter::LineFilter::new(request.filter.as_option()),
    }
}

fn log_page(page: page::Page) -> LogPage {
    LogPage {
        lines: page
            .lines
            .into_iter()
            .map(|line| LogLine {
                offset: line.offset,
                text: match String::from_utf8(line.text) {
                    Ok(text) => text,
                    Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
                },
                cut_from: line.cut_from,
                ..Default::default()
            })
            .collect(),
        start: page.start,
        end: page.end,
        scanned_to: page.scanned_to,
        size: page.size,
        partial: page.partial,
        ..Default::default()
    }
}

impl PublicRpc {
    /// The logs the client may open, Current run first.
    async fn list_logs(&self) -> Result<LogList, ConnectError> {
        let logs = self.sources.list().await.map_err(io_error)?;
        Ok(LogList {
            logs,
            ..Default::default()
        })
    }

    async fn read_page(&self, request: ReadLogRequest) -> Result<LogPage, ConnectError> {
        let path = self
            .sources
            .resolve(&request.log_id)
            .ok_or_else(log_not_found)?
            .path;
        let permit = Arc::clone(&self.page_reads)
            .acquire_owned()
            .await
            .map_err(|_| api_stopped())?;
        let request = page_request(&request);
        let page = crate::runtime::spawn_blocking(move || {
            let _permit = permit;
            page::read_page(&path, &request, page::SCAN_BUDGET)
        })
        .await
        .map_err(|_| ConnectError::internal("log read failed"))?
        .map_err(io_error)?;
        Ok(log_page(page))
    }

    /// Snapshots of one log's summary, ending with the finished one.
    ///
    /// A log the cache already holds answers with one snapshot. Otherwise a
    /// scan streams snapshots as it reads and caches the result when it reaches
    /// the end. The scan stops when the client drops the stream.
    async fn summary_stream(
        &self,
        request: SummarizeLogRequest,
    ) -> Result<ServiceStream<LogSummary>, ConnectError> {
        let resolved = self
            .sources
            .resolve(&request.log_id)
            .ok_or_else(log_not_found)?;
        let permit = Arc::clone(&self.summaries)
            .acquire_owned()
            .await
            .map_err(|_| api_stopped())?;
        let metadata = tokio::fs::metadata(&resolved.path)
            .await
            .map_err(io_error)?;
        if !metadata.is_file() {
            return Err(log_not_found());
        }
        let stamp = Stamp::of(&metadata);
        let tally = match self.cache.plan(&resolved.path, stamp, resolved.current) {
            Plan::Cached(tally) => {
                let done = tally.to_proto(stamp.size, true);
                return Ok(Box::pin(futures_lite::stream::once(Ok(done))));
            }
            Plan::Scan(tally) => tally,
        };
        let (tx, rx) = mpsc::channel(SNAPSHOT_BACKLOG);
        let cache = Arc::clone(&self.cache);
        crate::runtime::spawn_blocking(move || {
            let _permit = permit;
            scan_and_send(&resolved.path, stamp, tally, &cache, &tx);
        });
        Ok(Box::pin(futures_lite::stream::unfold(rx, |mut rx| async {
            let item = rx.recv().await?;
            Some((item, rx))
        })))
    }

    /// The logs of a bundle request, resolved and measured now.
    ///
    /// A log named twice is bundled once.
    async fn bundle_files(&self, log_ids: &[String]) -> Result<Vec<BundleFile>, ConnectError> {
        if log_ids.is_empty() {
            return Err(ConnectError::invalid_argument("select at least one log"));
        }
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        for id in log_ids {
            let path = self.sources.resolve(id).ok_or_else(log_not_found)?.path;
            if !seen.insert(path.clone()) {
                continue;
            }
            let metadata = tokio::fs::metadata(&path).await.map_err(io_error)?;
            let name = path.file_name().ok_or_else(log_not_found)?;
            if !metadata.is_file() {
                return Err(log_not_found());
            }
            files.push(BundleFile {
                name: name.to_string_lossy().into_owned(),
                size: metadata.len(),
                modified: metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |elapsed| elapsed.as_secs()),
                path,
            });
        }
        Ok(files)
    }

    /// Starts the streamed Log bundle, unless another bundle job is running.
    async fn start_download(
        &self,
        request: DownloadLogsRequest,
    ) -> Result<ServiceStream<DownloadEvent>, ConnectError> {
        let permit = download::try_start_job(&self.jobs)?;
        let files = self.bundle_files(&request.log_ids).await?;
        Ok(download::stream_bundle(
            files,
            download::bundle_name(chrono::Utc::now()),
            permit,
        ))
    }
}

/// Runs a summary scan, sending its snapshots, and caches a finished one.
fn scan_and_send(
    path: &std::path::Path,
    stamp: Stamp,
    tally: Tally,
    cache: &SummaryCache,
    tx: &mpsc::Sender<Result<LogSummary, ConnectError>>,
) {
    let outcome = summary::scan(path, tally, |tally| {
        let size = stamp.size.max(tally.offset());
        tx.blocking_send(Ok(tally.to_proto(size, false))).is_ok()
    });
    match outcome {
        Ok((tally, true)) => {
            let size = stamp.size.max(tally.offset());
            cache.store(path, stamp, tally.clone());
            let _ = tx.blocking_send(Ok(tally.to_proto(size, true)));
        }
        Ok((_, false)) => {}
        Err(error) => {
            let _ = tx.blocking_send(Err(io_error(error)));
        }
    }
}

impl Logs for PublicRpc {
    #[allow(clippy::manual_async_fn)]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn list<'a>(
        &'a self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, ListLogsRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<LogList> + Send + use<'a>>> + Send {
        async move { self.list_logs().await.and_then(connectrpc::Response::ok) }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn read<'a>(
        &'a self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ReadLogRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<LogPage> + Send + use<'a>>> + Send {
        let owned = request.to_owned_message();
        async move {
            self.read_page(owned)
                .await
                .and_then(connectrpc::Response::ok)
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn summarize(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, SummarizeLogRequest>,
    ) -> impl Future<
        Output = ServiceResult<ServiceStream<impl Encodable<LogSummary> + Send + use<>>>,
    > + Send {
        let owned = request.to_owned_message();
        async move {
            self.summary_stream(owned)
                .await
                .and_then(connectrpc::Response::stream_ok)
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn download(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, DownloadLogsRequest>,
    ) -> impl Future<
        Output = ServiceResult<ServiceStream<impl Encodable<DownloadEvent> + Send + use<>>>,
    > + Send {
        let owned = request.to_owned_message();
        async move {
            self.start_download(owned)
                .await
                .and_then(connectrpc::Response::stream_ok)
        }
    }
}

/// Entry points the `logs` criterion benchmark drives.
///
/// The handler's internals stay private; this module exists only under the
/// `bench` feature so the benchmark can time the same code the RPCs run.
#[cfg(feature = "bench")]
pub mod bench {
    use super::{bundle, filter, page, summary};
    pub use cadmus_api_proto::cadmus::api::public::v1::{LogFilter, LogLevel};
    use std::io;
    use std::path::Path;

    /// Matching lines of the newest page of `path`.
    ///
    /// # Errors
    ///
    /// Returns the I/O error of the file.
    pub fn last_page(path: &Path, limit: usize, filter: &LogFilter) -> io::Result<usize> {
        let request = page::Request {
            anchor: page::Anchor::End,
            direction: page::Direction::Backward,
            limit,
            filter: filter::LineFilter::new(Some(filter)),
        };
        page::read_page(path, &request, page::SCAN_BUDGET).map(|page| page.lines.len())
    }

    /// Matching lines of the whole file, read page by page from the end the
    /// way the viewer does while a filter has few matches.
    ///
    /// # Errors
    ///
    /// Returns the I/O error of the file.
    pub fn scan_all(path: &Path, filter: &LogFilter) -> io::Result<u64> {
        let mut anchor = page::Anchor::End;
        let mut matched = 0u64;
        loop {
            let request = page::Request {
                anchor,
                direction: page::Direction::Backward,
                limit: page::MAX_LIMIT,
                filter: filter::LineFilter::new(Some(filter)),
            };
            let page = page::read_page(path, &request, page::SCAN_BUDGET)?;
            matched += page.lines.len() as u64;
            if page.start == 0 {
                return Ok(matched);
            }
            anchor = page::Anchor::Cursor(page.start);
        }
    }

    /// Lines counted by one full summary scan of `path`.
    ///
    /// # Errors
    ///
    /// Returns the I/O error of the file.
    pub fn summarize(path: &Path) -> io::Result<u64> {
        let (tally, _) = summary::scan(path, summary::Tally::default(), |_| true)?;
        Ok(tally.to_proto(0, true).lines)
    }

    /// Deflate level Download uses on the device.
    pub const DEFLATE_LEVEL: i64 = bundle::DEFLATE_LEVEL;

    /// Bytes of zip that the first `bytes` bytes of `path` compress to at
    /// `deflate_level`.
    ///
    /// # Errors
    ///
    /// Returns the I/O error of the file.
    pub fn bundle_size(path: &Path, bytes: u64, deflate_level: i64) -> io::Result<u64> {
        struct Count(u64);
        impl io::Write for Count {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.0 += data.len() as u64;
                Ok(data.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let file = bundle::BundleFile {
            name: "cadmus.json".to_owned(),
            path: path.to_path_buf(),
            size: std::fs::metadata(path)?.len().min(bytes),
            modified: 0,
        };
        let read = std::sync::atomic::AtomicU64::new(0);
        bundle::write_bundle_with(&[file], Count(0), &read, deflate_level).map(|count| count.0)
    }
}
