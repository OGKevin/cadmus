//! Companion-only handler: the `cadmus.api.public.v1` services.
//!
//! [`PublicRpc`] owns the Logs state; its service logic and the log internals
//! live in [`super::logs`].

use super::logs::{LogSources, SummaryCache};
use cadmus_api_proto::cadmus::api::public::v1::LogsExt;
use connectrpc::Router;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Page reads that may run at once.
const MAX_PAGE_READS: usize = 2;

/// Summaries that may scan at once. A scan of a large log takes a while on a
/// Kobo, so a second one waits instead of competing for the flash.
const MAX_SUMMARIES: usize = 1;

/// Companion-only handler: the `cadmus.api.public.v1` services.
///
/// Its state is the logs it serves: the sources, the summary cache, and the
/// permits that bound page reads, summaries, and bundles.
pub(super) struct PublicRpc {
    pub(super) sources: Arc<LogSources>,
    pub(super) cache: Arc<SummaryCache>,
    pub(super) page_reads: Arc<Semaphore>,
    pub(super) summaries: Arc<Semaphore>,
    /// One permit: a Download holds it while it runs.
    pub(super) jobs: Arc<Semaphore>,
}

/// Add the Companion-only services to `router`.
pub(super) fn register(router: Router, sources: LogSources) -> Router {
    let logs = Arc::new(PublicRpc {
        sources: Arc::new(sources),
        cache: Arc::new(SummaryCache::default()),
        page_reads: Arc::new(Semaphore::new(MAX_PAGE_READS)),
        summaries: Arc::new(Semaphore::new(MAX_SUMMARIES)),
        jobs: Arc::new(Semaphore::new(1)),
    });
    LogsExt::register(logs, router)
}
