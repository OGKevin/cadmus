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
use std::thread;
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
}

struct UnlockInner {
    state: Mutex<UnlockState>,
    cv: Condvar,
    pins: AtomicUsize,
}

impl UnlockInner {
    fn new(paths: SoftSuspendPaths) -> Arc<Self> {
        let inner = Arc::new(Self {
            state: Mutex::new(UnlockState {
                due_at: None,
                held: false,
                paths,
                shutdown: false,
            }),
            cv: Condvar::new(),
            pins: AtomicUsize::new(0),
        });
        let worker = Arc::clone(&inner);
        thread::spawn(move || worker.run());
        inner
    }

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
                let unlocked = write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME);
                if self.pins.load(Ordering::SeqCst) != 0 {
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
            }
        }
    }

    fn pin(&self) {
        self.pins.fetch_add(1, Ordering::SeqCst);
        self.cancel_pending_unlock();
    }

    fn unpin(&self) {
        self.pins.fetch_sub(1, Ordering::SeqCst);
    }

    fn take_wake_lock(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let paths = state.paths.clone();
        state.due_at = None;
        self.cv.notify_one();
        drop(state);
        if write_sysfs_applied(&paths.wake_lock, WAKE_LOCK_NAME) {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.held = true;
        }
    }

    fn release_wake_lock_now(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let paths = state.paths.clone();
        state.due_at = None;
        self.cv.notify_one();
        drop(state);
        tracing::info!(
            wake_lock = WAKE_LOCK_NAME,
            "soft-suspend writing wake_unlock"
        );
        if write_sysfs_applied(&paths.wake_unlock, WAKE_LOCK_NAME) {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.held = false;
        }
    }

    fn schedule_unlock(&self, grace: Duration) {
        if grace.is_zero() {
            self.release_wake_lock_now();
            return;
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let due_at = Instant::now() + grace;
        state.due_at = Some(due_at);
        tracing::debug!(
            wake_lock = WAKE_LOCK_NAME,
            grace_secs = grace.as_secs_f32(),
            "soft-suspend last holder released; scheduling unlock"
        );
        self.cv.notify_one();
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
