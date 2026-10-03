//! Request cancellation policy for the shared HTTP client.
//!
//! # Example
//!
//! Long downloads and OTA deploys share a [`CancelFlag`] so the UI can abort
//! while retries still observe cancellation through [`RequestCancel`]:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use cadmus_core::http::{CancelFlag, Client};
//!
//! async fn download_with_abort(url: &str) -> Result<(), Box<dyn std::error::Error>> {
//!     let client = Client::new()?;
//!     let flag = CancelFlag::new();
//!     let flag_for_ui = flag.clone();
//!     // UI thread: flag_for_ui.request_cancel();
//!     let dest = Path::new("/tmp/staging.bin");
//!     let mut progress = |_: u64, _: u64| {};
//!     client
//!         .download(
//!             url,
//!             Some(0),
//!             dest,
//!             |u| client.get(u),
//!             &mut progress,
//!             Some(&flag),
//!         )
//!         .await?;
//!     Ok(())
//! }
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tokio_util::sync::CancellationToken;

const CANCEL_STATE_RUNNING: u8 = 0;
const CANCEL_STATE_CANCELLED: u8 = 1;
const CANCEL_STATE_COMMITTED: u8 = 2;

/// Shared cancel/commit gate for long-running downloads and deploys.
///
/// Cancellation wins until [`Self::try_commit`] succeeds. After a successful
/// commit transition, further cancel requests are ignored.
#[derive(Clone, Debug)]
pub struct CancelFlag {
    state: Arc<AtomicU8>,
    token: CancellationToken,
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelFlag {
    /// Creates a running cancel flag.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(CANCEL_STATE_RUNNING)),
            token: CancellationToken::new(),
        }
    }

    /// Token tripped when cancellation wins before commit.
    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Requests cancellation. No-op if the operation already committed.
    pub fn request_cancel(&self) {
        if self
            .state
            .compare_exchange(
                CANCEL_STATE_RUNNING,
                CANCEL_STATE_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.token.cancel();
        }
    }

    /// Returns `true` when cancellation won before commit.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == CANCEL_STATE_CANCELLED || self.token.is_cancelled()
    }

    /// Returns `true` while neither cancellation nor commit has been decided.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state.load(Ordering::Acquire) == CANCEL_STATE_RUNNING
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

/// Whether an in-flight request may still be abandoned.
///
/// A deployment crosses a point of no return once its new bundle is published:
/// cancelling past that point would leave the device with a half-written
/// KoboRoot. [`CancelFlag`] already encodes that (a committed flag is neither
/// cancelled nor running), so a bridged request stops firing on its own. This
/// type exists for callers that know they are *already* past that point and
/// must not attach a cancel token at all.
#[derive(Clone, Debug)]
pub(crate) enum RequestCancel {
    /// The request may be dropped at any await point.
    Cancellable(CancellationToken),
    /// The operation is committed; let the request run to completion.
    Committed,
}

impl RequestCancel {
    /// Bridges a [`CancelFlag`], the source of truth for cancellation and for
    /// the commit transition.
    ///
    /// Only a committed flag becomes [`RequestCancel::Committed`]; an
    /// already-cancelled flag stays [`RequestCancel::Cancellable`] so its token
    /// (already tripped) is observed and the request is abandoned.
    pub(crate) fn from_flag(flag: &CancelFlag) -> Self {
        if !flag.is_running() && !flag.is_cancelled() {
            return Self::Committed;
        }
        Self::Cancellable(flag.cancellation_token())
    }

    /// Returns `true` when a cancellation request is pending.
    pub(crate) fn is_cancelled(&self) -> bool {
        match self {
            Self::Cancellable(token) => token.is_cancelled(),
            Self::Committed => false,
        }
    }

    /// Fires when cancellation is requested, and never for a committed request.
    ///
    /// Lets callers `select!` on cancellation without special-casing the
    /// committed arm.
    pub(crate) async fn cancelled(&self) {
        match self {
            Self::Cancellable(token) => token.cancelled().await,
            Self::Committed => std::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_running_flag_starts_cancellable_and_trips_on_cancel() {
        let flag = CancelFlag::new();
        let cancel = RequestCancel::from_flag(&flag);
        assert!(matches!(cancel, RequestCancel::Cancellable(_)));
        assert!(!cancel.is_cancelled());

        flag.request_cancel();

        cancel.cancelled().await;
        assert!(cancel.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_committed_flag_never_trips() {
        let flag = CancelFlag::new();
        assert!(flag.try_commit());

        let cancel = RequestCancel::from_flag(&flag);
        assert!(matches!(cancel, RequestCancel::Committed));

        flag.request_cancel();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!cancel.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancelled_flag_stays_cancellable() {
        let flag = CancelFlag::new();
        flag.request_cancel();

        let cancel = RequestCancel::from_flag(&flag);
        assert!(matches!(cancel, RequestCancel::Cancellable(_)));
        assert!(cancel.is_cancelled(), "a cancelled flag must be observed");
        cancel.cancelled().await;
    }
}
