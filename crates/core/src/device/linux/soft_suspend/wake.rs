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
//! This module does **not** write `/sys/power/autosleep` or drive the status
//! LED — that is [`super::autosleep`]. Cadmus suspend / exit gating is
//! [`Kind::Full`](crate::device::inhibitor::Kind::Full), not SoftSuspend.
//!
//! # Architecture
//!
//! ```text
//!   acquire("wifi") ──► LeaseTracker ──► WakeLockArmer (LeaseObserver)
//!                              │                    │
//!                              │         on_first_acquire → wake_lock
//!                              │         on_last_release  → schedule unlock
//!                              ▼
//!                        Lease (RAII drop releases)
//!
//!   UnlockInner (background thread)
//!        waits on due_at ──► wake_unlock when grace elapses and no pins
//! ```

use super::WAKE_LOCK_NAME;
use super::paths::{SoftSuspendPaths, SysfsWrite, write_sysfs};
use crate::lease::{Lease, LeaseName, LeaseObserver, LeaseTracker};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

fn write_sysfs_applied(path: &Path, value: &str) -> bool {
    match write_sysfs(path, value) {
        Ok(SysfsWrite::Written) => {
            tracing::debug!(path = %path.display(), value, "wrote soft-suspend sysfs");
            true
        }
        Ok(SysfsWrite::Missing) => {
            tracing::debug!(path = %path.display(), value, "soft-suspend sysfs path missing");
            true
        }
        Err(error) => {
            tracing::warn!(error = %error, value, "failed to write soft-suspend sysfs");
            false
        }
    }
}

struct UnlockState {
    due_at: Option<Instant>,
    held: bool,
    paths: SoftSuspendPaths,
    shutdown: bool,
    paused: bool,
}

struct UnlockInner {
    state: Mutex<UnlockState>,
    cv: Condvar,
    pins: AtomicUsize,
    writers: AtomicUsize,
}

impl UnlockInner {
    fn new(paths: SoftSuspendPaths) -> Arc<Self> {
        let inner = Arc::new(Self {
            state: Mutex::new(UnlockState {
                due_at: None,
                held: false,
                paths,
                shutdown: false,
                paused: false,
            }),
            cv: Condvar::new(),
            pins: AtomicUsize::new(0),
            writers: AtomicUsize::new(0),
        });
        let worker = Arc::clone(&inner);
        crate::runtime::spawn_blocking(move || worker.run());
        inner
    }

    /// Waits for the release grace to elapse, then unlocks.
    ///
    /// The state mutex is never held across a sysfs write. Every other writer
    /// in this file releases it first, because a write that blocks while
    /// holding the lock would serialise every other wake-lock operation behind
    /// it.
    fn run(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.shutdown {
                break;
            }

            if state.paused {
                state.due_at = None;
                state = self.cv.wait(state).unwrap_or_else(|e| e.into_inner());
                continue;
            }

            let Some(due) = state.due_at else {
                state = self.cv.wait(state).unwrap_or_else(|e| e.into_inner());
                continue;
            };

            let now = Instant::now();
            if due > now {
                let wait = due - now;
                let (guard, _) = self
                    .cv
                    .wait_timeout(state, wait)
                    .unwrap_or_else(|e| e.into_inner());
                state = guard;
                continue;
            }

            if state.due_at.is_some_and(|d| d <= Instant::now()) {
                if self.pins.load(Ordering::SeqCst) != 0 {
                    state.due_at = None;
                    continue;
                }
                let paths = state.paths.clone();
                state.due_at = None;
                tracing::info!(
                    wake_lock = WAKE_LOCK_NAME,
                    "soft-suspend release grace elapsed; unlocking"
                );
                drop(state);
                self.begin_sysfs_write();
                let unlocked = write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME);
                state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.paused {
                    state.held = false;
                } else if self.pins.load(Ordering::SeqCst) != 0 {
                    tracing::debug!(
                        wake_lock = WAKE_LOCK_NAME,
                        "soft-suspend re-arm after unlock raced with acquire"
                    );
                    if write_sysfs_applied(&paths.wake_lock, WAKE_LOCK_NAME) {
                        state.held = true;
                    } else if unlocked {
                        state.held = false;
                    }
                } else if unlocked {
                    state.held = false;
                }
                drop(state);
                self.end_sysfs_write();
                state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            }
        }
    }

    fn pin(&self) {
        self.pins.fetch_add(1, Ordering::SeqCst);
        self.cancel_pending_unlock();
    }

    fn unpin(&self) {
        let _state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.pins.fetch_sub(1, Ordering::SeqCst);
        self.cv.notify_all();
    }

    fn begin_sysfs_write(&self) {
        let _state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.writers.fetch_add(1, Ordering::SeqCst);
    }

    fn end_sysfs_write(&self) {
        let _state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.writers.fetch_sub(1, Ordering::SeqCst);
        self.cv.notify_all();
    }

    /// Blocks until no lease acquisition or sysfs write is in flight.
    ///
    /// Pins cover tracker acquisitions and writers cover sysfs writes. Both
    /// decrement under the state mutex and notify the condvar, so the predicate
    /// cannot change between the check and [`Condvar::wait`].
    fn wait_for_sysfs_writers(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while self.pins.load(Ordering::SeqCst) != 0 || self.writers.load(Ordering::SeqCst) != 0 {
            state = self.cv.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn commit_lock_write(&self, paths: &SoftSuspendPaths, locked: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.paused {
            drop(state);
            let unlocked = write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME);
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if unlocked {
                state.held = false;
            } else if locked {
                state.held = true;
            }
        } else if locked {
            state.held = true;
        }
    }

    fn commit_unlock_write(&self, unlocked: bool) {
        if unlocked {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.held = false;
        }
    }

    fn take_wake_lock(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.paused {
            return;
        }
        let paths = state.paths.clone();
        state.due_at = None;
        self.cv.notify_one();
        drop(state);
        let locked = write_sysfs_applied(&paths.wake_lock, WAKE_LOCK_NAME);
        self.commit_lock_write(&paths, locked);
    }

    fn release_wake_lock_now(&self) {
        self.begin_sysfs_write();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.paused {
            drop(state);
            self.end_sysfs_write();
            return;
        }
        let paths = state.paths.clone();
        state.due_at = None;
        self.cv.notify_one();
        drop(state);
        tracing::debug!(
            wake_lock = WAKE_LOCK_NAME,
            "soft-suspend writing wake_unlock"
        );
        self.commit_unlock_write(write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME));
        self.end_sysfs_write();
    }

    fn schedule_unlock(&self, grace: Duration) {
        if grace.is_zero() {
            self.release_wake_lock_now();
            return;
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.paused {
            state.due_at = None;
            self.cv.notify_one();
            return;
        }
        let due_at = Instant::now() + grace;
        state.due_at = Some(due_at);
        tracing::debug!(
            wake_lock = WAKE_LOCK_NAME,
            grace_secs = grace.as_secs_f32(),
            "soft-suspend last holder released; scheduling unlock"
        );
        self.cv.notify_one();
    }

    fn pause(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.paused {
                return;
            }
            state.paused = true;
            state.due_at = None;
            self.cv.notify_all();
        }

        self.wait_for_sysfs_writers();
        self.begin_sysfs_write();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.paused {
            drop(state);
            self.end_sysfs_write();
            return;
        }
        let paths = state.paths.clone();
        drop(state);
        tracing::debug!(
            wake_lock = WAKE_LOCK_NAME,
            "soft-suspend dropping kernel wake lock"
        );
        let unlocked = write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME);
        self.commit_unlock_write(unlocked);
        self.end_sysfs_write();
    }

    fn resume(&self, holders_alive: bool) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.paused {
                return;
            }
            state.paused = false;
            self.cv.notify_all();
        }

        if !holders_alive {
            return;
        }

        self.wait_for_sysfs_writers();
        self.begin_sysfs_write();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.paused {
            drop(state);
            self.end_sysfs_write();
            return;
        }
        let paths = state.paths.clone();
        state.due_at = None;
        drop(state);
        tracing::debug!(
            wake_lock = WAKE_LOCK_NAME,
            "soft-suspend retaking kernel wake lock"
        );
        let locked = write_sysfs_applied(&paths.wake_lock, WAKE_LOCK_NAME);
        self.commit_lock_write(&paths, locked);
        self.end_sysfs_write();
    }

    fn cancel_pending_unlock(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.due_at.take().is_some() {
            tracing::debug!(
                wake_lock = WAKE_LOCK_NAME,
                "soft-suspend pending unlock cancelled"
            );
        }
        self.cv.notify_one();
    }

    fn is_held(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).held
    }

    fn shutdown(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let paths = state.paths.clone();
        let should_unlock = state.held;
        state.shutdown = true;
        state.due_at = None;
        state.held = false;
        self.cv.notify_one();
        drop(state);
        if should_unlock {
            tracing::info!(
                wake_lock = WAKE_LOCK_NAME,
                "soft-suspend session teardown; unlocking"
            );
            write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME);
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
        self.unlock.shutdown();
    }
}

impl LeaseObserver for WakeLockArmer {
    fn on_first_acquire(&self, name: &LeaseName) {
        tracing::debug!(name = %name, wake_lock = WAKE_LOCK_NAME, "soft-suspend first holder");
        self.unlock.take_wake_lock();
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
        Arc::new(Self {
            tracker: LeaseTracker::with_observer(Arc::clone(&armer) as Arc<dyn LeaseObserver>),
            armer,
            autosleep_grace,
        })
    }

    pub(crate) fn acquire(&self, name: impl Into<LeaseName>) -> Lease {
        let name = name.into();
        self.armer.unlock.pin();
        let lease = self.tracker.acquire(name);
        self.armer.unlock.unpin();
        lease
    }

    pub(crate) fn len(&self) -> usize {
        self.tracker.len()
    }

    pub(crate) fn holders(&self) -> Vec<LeaseName> {
        self.tracker.holders()
    }

    /// Releases `cadmus` and suppresses wake-lock writes until [`Self::resume`].
    ///
    /// Lease tracking stays active, so holders acquired or dropped while paused
    /// are still reflected when the lock is restored.
    pub(crate) fn pause(&self) {
        self.armer.unlock.pause();
    }

    /// Ends suppression and retakes `cadmus` when any lease remains.
    pub(crate) fn resume(&self) {
        self.armer.unlock.resume(!self.tracker.is_empty());
    }

    pub(crate) fn set_autosleep_grace(&self, grace: Duration) {
        *self
            .autosleep_grace
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = grace;
        self.armer.unlock.cancel_pending_unlock();
        if self.tracker.is_empty() && self.armer.unlock.is_held() {
            self.armer.schedule_unlock(&LeaseName::from("grace-update"));
        }
    }
}
