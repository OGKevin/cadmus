//! Linux `cadmus` wake-lock tracking for SoftSuspend-kind leases.
//!
//! Cadmus features acquire **named leases** so the kernel knows the device is
//! busy. On Linux those leases collapse onto a single shared wake lock named
//! [`WAKE_LOCK_NAME`] (`"cadmus"`) in `/sys/power/wake_lock` / `wake_unlock`.
//!
//! # Behaviour
//!
//! - **First lease (0 → 1 holders):** write `cadmus` to `wake_lock`.
//! - **Nested leases:** holder count increases; the wake lock stays taken.
//! - **Last lease dropped (1 → 0):** unlock immediately or after release grace.
//! - **Re-acquire during grace:** cancel the pending unlock and keep the lock.
//! - **Cadmus suspend sleep window:** pause releases the lock immediately,
//!   bypassing grace, and suppresses acquire/release writes while holder
//!   accounting stays live. Resume re-locks only when holders remain.
//!
//! All hardware writes go through one [`UnlockInner::reconcile`] loop. The loop
//! computes demand from live Full holders, suppression, leases, and the grace
//! deadline, then serializes one sysfs writer at a time. A completed write is
//! the only evidence that `held` changed: a failed write returns a typed error
//! instead of pretending the intended state was applied.
//!
//! # Architecture
//!
//! ```text
//!   acquire("wifi") ──► LeaseTracker ──► WakeLockArmer (LeaseObserver)
//!                              │                    │
//!                       │              on_first_acquire → cancel grace
//!                       │              confirm_leases → reconcile
//!                              │         on_last_release  → deadline → reconcile
//!                              ▼
//!                        Lease (RAII drop releases)
//!
//!   UnlockInner (background thread)
//!        waits on due_at ──► grace expiry and bounded restore retries
//! ```

use super::WAKE_LOCK_NAME;
use super::paths::{SoftSuspendPaths, SysfsWrite, SysfsWriteError, write_sysfs_blocking};
use crate::device::inhibitor::InhibitorError;
use crate::device::inhibitor::soft_suspend::{
    SuppressionEpoch, WakeLockPauseOutcome, WakeLockResumeOutcome,
};
use crate::lease::{Lease, LeaseName, LeaseObserver, LeaseTracker, WeakLeaseTracker};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// Delayed attempts after a failed wake-lock restore.
const RECOVERY_DELAYS: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
];

/// Point at which the retry budget is replenished by an explicit operation.
const RECOVERY_ATTEMPTS: usize = RECOVERY_DELAYS.len();

/// Test seam for deterministic sysfs failures and in-flight write barriers.
#[cfg(all(test, target_os = "linux"))]
pub(crate) type WakeWriteHook =
    Arc<dyn Fn(&Path, &str) -> Result<SysfsWrite, SysfsWriteError> + Send + Sync + 'static>;

/// One wake-control request whose response resolves after hardware reconciliation.
enum WakeRequest {
    Acquire(oneshot::Sender<Result<bool, SysfsWriteError>>),
    Pause(oneshot::Sender<WakeLockPauseOutcome>),
    Resume {
        grace: Duration,
        reply: oneshot::Sender<WakeLockResumeOutcome>,
    },
    Shutdown(oneshot::Sender<Result<(), SysfsWriteError>>),
}

struct UnlockState {
    /// Next grace expiry or restore retry deadline.
    due_at: Option<Instant>,
    /// Last kernel state confirmed by a completed write.
    held: bool,
    suppressed: bool,
    suppress_epoch: u64,
    /// While true, another thread owns the sysfs write and only it may clear it.
    writing: bool,
    paths: SoftSuspendPaths,
    shutdown: bool,
    recovery_remaining: usize,
}

struct UnlockInner {
    state: Mutex<UnlockState>,
    requests: mpsc::UnboundedSender<WakeRequest>,
    cv: Condvar,
    pins: AtomicUsize,
    tracker: OnceLock<WeakLeaseTracker>,
    full_tracker: OnceLock<WeakLeaseTracker>,
    #[cfg(all(test, target_os = "linux"))]
    write_hook: Mutex<Option<WakeWriteHook>>,
}

impl UnlockInner {
    fn new(paths: SoftSuspendPaths) -> Arc<Self> {
        let (requests, control) = mpsc::unbounded_channel();
        let inner = Arc::new(Self {
            requests,
            state: Mutex::new(UnlockState {
                due_at: None,
                held: false,
                suppressed: false,
                suppress_epoch: 0,
                writing: false,
                paths,
                shutdown: false,
                recovery_remaining: 0,
            }),
            cv: Condvar::new(),
            pins: AtomicUsize::new(0),
            tracker: OnceLock::new(),
            full_tracker: OnceLock::new(),
            #[cfg(all(test, target_os = "linux"))]
            write_hook: Mutex::new(None),
        });
        let worker = Arc::clone(&inner);
        crate::runtime::spawn_blocking(move || worker.run());
        let worker = Arc::clone(&inner);
        crate::runtime::spawn_blocking(move || worker.run_requests(control));
        inner
    }

    #[cfg(all(test, target_os = "linux"))]
    fn set_write_hook(&self, hook: WakeWriteHook) {
        *self.write_hook.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    fn write(&self, path: &Path) -> Result<SysfsWrite, SysfsWriteError> {
        #[cfg(all(test, target_os = "linux"))]
        if let Some(hook) = self
            .write_hook
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return hook(path, WAKE_LOCK_NAME);
        }
        write_sysfs_blocking(path, WAKE_LOCK_NAME)
    }

    /// Waits for grace deadlines, then reconciles demand or retries a failed
    /// restore autonomously. State locks are never held across a sysfs write.
    fn run(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.shutdown {
                break;
            }
            let Some(due) = state.due_at else {
                state = self.cv.wait(state).unwrap_or_else(|e| e.into_inner());
                continue;
            };
            let now = Instant::now();
            if due > now {
                let (guard, _) = self
                    .cv
                    .wait_timeout(state, due - now)
                    .unwrap_or_else(|e| e.into_inner());
                state = guard;
                continue;
            }
            drop(state);
            match self.reconcile() {
                Ok(_) => {
                    state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.recovery_remaining = 0;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "soft-suspend wake-lock write failed");
                    self.schedule_recovery();
                    state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                }
            }
        }
    }

    fn pin(&self) {
        self.pins.fetch_add(1, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.due_at = None;
        self.cv.notify_all();
    }

    fn unpin(&self) {
        let _state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.pins.fetch_sub(1, Ordering::SeqCst);
        self.cv.notify_all();
    }

    fn has_holders(&self) -> bool {
        self.pins.load(Ordering::SeqCst) != 0
            || self
                .tracker
                .get()
                .is_some_and(|tracker| !tracker.is_empty())
    }

    fn has_full_holders(&self) -> bool {
        self.full_tracker
            .get()
            .is_some_and(|tracker| !tracker.is_empty())
    }

    fn bind_tracker(&self, tracker: &LeaseTracker) {
        let _ = self.tracker.set(tracker.downgrade());
    }

    fn set_full_tracker(&self, tracker: WeakLeaseTracker) {
        let _ = self.full_tracker.set(tracker);
    }

    /// Computes kernel demand and clears suppression before waiting on I/O.
    ///
    /// Full acquisition invalidates the pause epoch here, while its intent is
    /// live, rather than relying on a holder count sampled after the write.
    ///
    /// A “past epoch” is a generation recorded for an older sleep leg, not a
    /// timestamp. It is no longer current once suppression ends or its value
    /// changes. [`Self::wake_lock_epoch_is_current`] checks both state and
    /// generation, so a Full interruption remains visible even if the Full
    /// holder disappeared before the next poll. Cleanup is separate: consuming
    /// the cycle's saved epoch with `Option::take` limits restoration to one call
    /// for that sleep leg.
    fn compute_desired(&self, state: &mut UnlockState) -> bool {
        if state.shutdown {
            state.due_at = None;
            return false;
        }
        if self.has_full_holders() {
            if state.suppressed {
                state.suppressed = false;
                state.suppress_epoch += 1;
            }
            state.due_at = None;
            return true;
        }
        if state.suppressed {
            state.due_at = None;
            return false;
        }
        if self.has_holders() {
            state.due_at = None;
            return true;
        }
        if !state.held {
            state.due_at = None;
            return false;
        }
        if state.due_at.is_some_and(|due| due > Instant::now()) {
            return true;
        }
        state.due_at = None;
        false
    }

    /// Drives the confirmed kernel state towards live demand.
    ///
    /// Only one caller writes at a time. After each write the loop re-reads
    /// holder and suppression state, because both may have changed while the
    /// mutex was dropped. A missing node on this live Linux backend is an
    /// error, not a successful no-op.
    fn reconcile(&self) -> Result<bool, SysfsWriteError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            while state.writing {
                state = self.cv.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            let desired = self.compute_desired(&mut state);
            self.cv.notify_all();
            if desired == state.held {
                return Ok(state.held);
            }
            state.writing = true;
            let path = if desired {
                state.paths.wake_lock.clone()
            } else {
                state.paths.wake_unlock.clone()
            };
            drop(state);
            let result = self.write(&path);
            state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.writing = false;
            match result {
                Ok(SysfsWrite::Written) => {
                    state.held = desired;
                    self.cv.notify_all();
                }
                Ok(SysfsWrite::Missing) => {
                    self.cv.notify_all();
                    return Err(SysfsWriteError::missing(&path));
                }
                Err(error) => {
                    self.cv.notify_all();
                    return Err(error);
                }
            }
        }
    }

    /// Arms the next bounded retry, or records budget exhaustion.
    ///
    /// Public pause/resume and acquisition replenish the budget; the grace
    /// worker performs the delayed attempts without any further API calls.
    fn schedule_recovery(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.shutdown {
            return;
        }
        if state.recovery_remaining == 0 {
            tracing::error!(
                wake_lock = WAKE_LOCK_NAME,
                "soft-suspend wake-lock restore retries exhausted; mismatch retained"
            );
            state.due_at = None;
            self.cv.notify_all();
            return;
        }
        state.recovery_remaining -= 1;
        let attempt = RECOVERY_ATTEMPTS - state.recovery_remaining - 1;
        let delay = RECOVERY_DELAYS[attempt];
        state.due_at = Some(Instant::now() + delay);
        tracing::debug!(
            attempt = attempt + 1,
            delay_ms = delay.as_millis(),
            "soft-suspend wake-lock restore retry scheduled"
        );
        self.cv.notify_all();
    }

    fn replenish_recovery(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.recovery_remaining = RECOVERY_ATTEMPTS;
    }

    fn run_requests(&self, mut requests: mpsc::UnboundedReceiver<WakeRequest>) {
        while let Some(request) = requests.blocking_recv() {
            match request {
                WakeRequest::Acquire(reply) => {
                    self.replenish_recovery();
                    let result = self.reconcile();
                    if let Err(error) = &result {
                        tracing::warn!(error = %error, "soft-suspend wake-lock acquisition failed");
                        self.schedule_recovery();
                    }
                    let _ = reply.send(result);
                }
                WakeRequest::Pause(reply) => {
                    let _ = reply.send(self.pause_now());
                }
                WakeRequest::Resume { grace, reply } => {
                    let _ = reply.send(self.resume_now(grace));
                }
                WakeRequest::Shutdown(reply) => {
                    let result = self.shutdown_now();
                    let _ = reply.send(result);
                    break;
                }
            }
        }
    }

    /// Arms the grace deadline after the last holder releases.
    ///
    /// A write in flight may still confirm `held` after the holder is gone,
    /// so the deadline is kept for it; only a settled unheld state clears it.
    fn schedule_unlock(&self, grace: Duration) {
        let zero = grace.is_zero();
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.suppressed || (!state.held && !state.writing) || self.has_holders() {
                state.due_at = None;
                self.cv.notify_all();
                return;
            }
            state.recovery_remaining = 0;
            state.due_at = if zero {
                Some(Instant::now())
            } else {
                Some(Instant::now() + grace)
            };
            self.cv.notify_all();
        }
    }

    async fn request<T: Send + 'static>(
        &self,
        make_request: impl FnOnce(oneshot::Sender<T>) -> WakeRequest + Send + 'static,
    ) -> Option<T> {
        let (reply, outcome) = oneshot::channel();
        self.requests.send(make_request(reply)).ok()?;
        outcome.await.ok()
    }

    async fn acquire(&self) -> Option<Result<bool, SysfsWriteError>> {
        self.request(WakeRequest::Acquire).await
    }

    async fn pause(&self) -> WakeLockPauseOutcome {
        self.request(WakeRequest::Pause)
            .await
            .unwrap_or(WakeLockPauseOutcome::UnlockFailed)
    }

    async fn resume(&self, grace: Duration) -> WakeLockResumeOutcome {
        self.request(move |reply| WakeRequest::Resume { grace, reply })
            .await
            .unwrap_or(WakeLockResumeOutcome::WorkerFailed)
    }

    fn pause_now(&self) -> WakeLockPauseOutcome {
        let epoch = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if self.has_full_holders() {
                return WakeLockPauseOutcome::BlockedByFull;
            }
            if state.suppressed {
                state.suppress_epoch
            } else {
                state.suppressed = true;
                state.suppress_epoch += 1;
                state.suppress_epoch
            }
        };
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.due_at = None;
            state.recovery_remaining = 0;
            self.cv.notify_all();
        }
        match self.reconcile() {
            Ok(_) => {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if !state.suppressed || state.suppress_epoch != epoch {
                    return WakeLockPauseOutcome::BlockedByFull;
                }
                if state.held || state.writing {
                    return WakeLockPauseOutcome::UnlockFailed;
                }
                WakeLockPauseOutcome::Paused(SuppressionEpoch(epoch))
            }
            Err(error) => {
                tracing::warn!(error = %error, "soft-suspend wake-lock unlock failed");
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.suppressed && state.suppress_epoch == epoch {
                    state.suppressed = false;
                    state.suppress_epoch += 1;
                }
                drop(state);
                self.replenish_recovery();
                self.schedule_recovery();
                WakeLockPauseOutcome::UnlockFailed
            }
        }
    }

    fn wake_lock_epoch_is_current(&self, epoch: SuppressionEpoch) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.suppressed && state.suppress_epoch == epoch.0
    }

    fn resume_now(&self, grace: Duration) -> WakeLockResumeOutcome {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.suppressed {
                state.suppressed = false;
                state.suppress_epoch += 1;
            }
            if !state.suppressed
                && state.held
                && !self.has_holders()
                && !grace.is_zero()
                && state.due_at.is_none()
            {
                state.due_at = Some(Instant::now() + grace);
            }
            state.recovery_remaining = RECOVERY_ATTEMPTS;
            self.cv.notify_all();
        }
        match self.reconcile() {
            Ok(true) => WakeLockResumeOutcome::Restored,
            Ok(false) => WakeLockResumeOutcome::Unheld,
            Err(error) => {
                tracing::warn!(error = %error, "soft-suspend wake-lock restore failed");
                self.schedule_recovery();
                WakeLockResumeOutcome::RestoreFailed
            }
        }
    }

    fn cancel_pending_unlock(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.due_at.take().is_some() {
            tracing::debug!(
                wake_lock = WAKE_LOCK_NAME,
                "soft-suspend pending unlock cancelled"
            );
        }
        self.cv.notify_all();
    }

    /// True when the lock is confirmed held or a write that may confirm it is in flight.
    fn may_be_held(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.held || state.writing
    }

    fn shutdown_now(&self) -> Result<(), SysfsWriteError> {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.shutdown = true;
            state.due_at = None;
            state.suppressed = false;
            self.cv.notify_all();
        }
        match self.reconcile() {
            Ok(_) => Ok(()),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    wake_lock = WAKE_LOCK_NAME,
                    "soft-suspend session teardown could not confirm unlock"
                );
                Err(error)
            }
        }
    }
}

struct WakeLockArmer {
    unlock: Arc<UnlockInner>,
    autosleep_grace: Arc<Mutex<Duration>>,
}

impl WakeLockArmer {
    fn schedule_unlock(&self, name: &LeaseName) {
        let grace = *self
            .autosleep_grace
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if grace.is_zero() {
            tracing::debug!(
                name = %name,
                wake_lock = WAKE_LOCK_NAME,
                "soft-suspend last holder released; unlocking immediately"
            );
        } else {
            tracing::debug!(
                name = %name,
                wake_lock = WAKE_LOCK_NAME,
                grace_secs = grace.as_secs_f32(),
                "soft-suspend last holder released; scheduling unlock"
            );
        }
        self.unlock.schedule_unlock(grace);
    }
}

impl Drop for WakeLockArmer {
    fn drop(&mut self) {
        let (reply, _outcome) = oneshot::channel();
        let _ = self.unlock.requests.send(WakeRequest::Shutdown(reply));
    }
}

impl LeaseObserver for WakeLockArmer {
    fn on_first_acquire(&self, name: &LeaseName) {
        tracing::debug!(name = %name, wake_lock = WAKE_LOCK_NAME, "soft-suspend first holder");
        self.unlock.cancel_pending_unlock();
    }

    fn on_last_release(&self, name: &LeaseName) {
        self.schedule_unlock(name);
    }
}

/// Linux wake-lock backend for SoftSuspend-kind leases.
pub(crate) struct WakeLock {
    tracker: LeaseTracker,
    armer: Arc<WakeLockArmer>,
    autosleep_grace: Arc<Mutex<Duration>>,
}

impl WakeLock {
    pub(crate) fn new(paths: SoftSuspendPaths) -> Arc<Self> {
        let unlock = UnlockInner::new(paths);
        let autosleep_grace = Arc::new(Mutex::new(Duration::ZERO));
        let armer = Arc::new(WakeLockArmer {
            unlock,
            autosleep_grace: Arc::clone(&autosleep_grace),
        });
        let tracker = LeaseTracker::with_observer(Arc::clone(&armer) as Arc<dyn LeaseObserver>);
        armer.unlock.bind_tracker(&tracker);
        Arc::new(Self {
            tracker,
            armer,
            autosleep_grace,
        })
    }

    pub(crate) fn register(&self, name: impl Into<LeaseName>) -> Lease {
        let name = name.into();
        self.armer.unlock.pin();
        let lease = self.tracker.acquire(name);
        self.armer.unlock.unpin();
        lease
    }

    /// Awaits worker reconciliation and preserves logical leases for retryable writes.
    pub(crate) async fn confirm_leases(&self) -> Result<(), InhibitorError> {
        match self.armer.unlock.acquire().await {
            Some(Ok(_) | Err(_)) => Ok(()),
            None => Err(InhibitorError::WorkerFailed),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tracker.len()
    }

    pub(crate) fn holders(&self) -> Vec<LeaseName> {
        self.tracker.holders()
    }

    /// Tracks Full holders so pause refuses to suppress their protection.
    pub(crate) fn set_full_tracker(&self, tracker: WeakLeaseTracker) {
        self.armer.unlock.set_full_tracker(tracker);
    }

    /// Releases `cadmus` and suppresses wake-lock writes until [`Self::resume`].
    ///
    /// A transient Full interruption returns `BlockedByFull` even when the
    /// holder has already gone away: Full registration invalidates the pause
    /// epoch before its release can clear it.
    pub(crate) async fn pause(&self) -> WakeLockPauseOutcome {
        self.armer.unlock.pause().await
    }

    /// Ends suppression and reports the confirmed restore outcome.
    pub(crate) async fn resume(&self) -> WakeLockResumeOutcome {
        let grace = *self
            .autosleep_grace
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.armer.unlock.resume(grace).await
    }

    pub(crate) fn wake_lock_epoch_is_current(&self, epoch: SuppressionEpoch) -> bool {
        self.armer.unlock.wake_lock_epoch_is_current(epoch)
    }

    pub(crate) async fn shutdown(&self) -> Result<(), InhibitorError> {
        let (reply, outcome) = oneshot::channel();
        self.armer
            .unlock
            .requests
            .send(WakeRequest::Shutdown(reply))
            .map_err(|_| InhibitorError::WorkerFailed)?;
        outcome
            .await
            .map_err(|_| InhibitorError::WorkerFailed)?
            .map_err(|_| InhibitorError::WakeLockWriteFailed)
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn set_write_hook(&self, hook: WakeWriteHook) {
        self.armer.unlock.set_write_hook(hook);
    }

    pub(crate) fn set_autosleep_grace(&self, grace: Duration) {
        *self
            .autosleep_grace
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = grace;
        if !self.armer.unlock.has_holders() {
            self.armer.unlock.cancel_pending_unlock();
        }
        if self.tracker.is_empty() && self.armer.unlock.may_be_held() {
            self.armer.schedule_unlock(&LeaseName::from("grace-update"));
        }
    }
}
