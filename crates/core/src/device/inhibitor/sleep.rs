//! Sleep-inhibit holder tracking and nested wake lock.
//!
//! [`Kind::Sleep`](super::Kind::Sleep) is a **named holder set** plus a nested
//! SoftSuspend lease. Callers still acquire through
//! [`Inhibitor::acquire`](super::Inhibitor::acquire); this module owns the
//! tracker and the first/last-holder transitions.
//!
//! # Architecture
//!
//! ```text
//!   acquire("main-menu") ──► LeaseTracker ──► SleepInhibitResources
//!                              │                    │
//!                              │         0 → 1  on_first_acquire
//!                              │              └─ SoftSuspend lease "sleep-inhibit"
//!                              │         1 → 0  on_last_release
//!                              │              ├─ drop wake lease
//!                              │              └─ on_last_release notifier
//!                              ▼
//!                         Lease (RAII; InhibitorGuard::sleep wraps it)
//! ```
//!
//! Named Sleep holders (`"main-menu"`, tests, …) are **not** SoftSuspend names.
//! The nested wake lock is always [`SLEEP_INHIBIT_NAME`] (`"sleep-inhibit"`) so
//! the kernel `cadmus` lock stays taken for the whole Sleep window, even if no
//! other SoftSuspend lease is held. An explicit
//! [`start_cycle`](crate::device::suspend::start_cycle) pauses that nested lock
//! for the cycle so DeepIdle / Classic sleep is not pinned.
//!
//! Sleep drives no status LED. Battery gating happens in
//! [`Inhibitor`](super::Inhibitor) **before**
//! [`SleepInhibitState::acquire`]; this module does not read capacity.
//!
//! # Last-holder notifier
//!
//! Kobo registers a last-release notifier that posts
//! [`Event::InhibitCleared`](crate::view::Event::InhibitCleared) with
//! [`Kind::Sleep`](super::Kind::Sleep), so the suspend orchestrator can flush a
//! Deferred Auto Suspend on the main loop. The callback runs on whichever thread
//! drops the last guard, and every current Sleep holder releases on the main
//! loop (Companion's public API stop, the PIN lockout, the test-only main
//! menu), so it must only enqueue. Only a [`Kind::Full`](super::Kind::Full) OTA
//! release runs off-thread.

use super::soft_suspend::SoftSuspendKind;
use crate::lease::{Lease, LeaseName, LeaseObserver, LeaseTracker};
use std::sync::{Arc, Mutex};

/// SoftSuspend lease name taken for the nested wake lock while any Sleep holder
/// is active. Distinct from caller names such as `"main-menu"`.
const SLEEP_INHIBIT_NAME: &str = "sleep-inhibit";

/// Nested `"sleep-inhibit"` lease plus whether an explicit cycle paused it.
struct SleepWake {
    lease: Option<Lease>,
    paused: bool,
}

/// Shared side-effect state for the Sleep 0→1 / 1→0 transitions.
///
/// Held by the [`LeaseObserver`] and, on kobo/tests/docsrs, by
/// [`SleepInhibitState`] so the last-release notifier can be installed after
/// construction.
struct SleepInhibitResources {
    soft_suspend: Arc<dyn SoftSuspendKind>,
    wake: Mutex<SleepWake>,
    /// Optional callback after the last Sleep holder drops (Kobo posts
    /// [`Event::InhibitCleared`](crate::view::Event::InhibitCleared) with
    /// [`Kind::Sleep`](super::Kind::Sleep)).
    on_last_release: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl SleepInhibitResources {
    fn new(soft_suspend: Arc<dyn SoftSuspendKind>) -> Arc<Self> {
        Arc::new(Self {
            soft_suspend,
            wake: Mutex::new(SleepWake {
                lease: None,
                paused: false,
            }),
            on_last_release: Mutex::new(None),
        })
    }

    /// Replaces the last-holder callback. The previous callback is dropped.
    #[cfg(any(test, feature = "kobo", docsrs))]
    fn set_on_last_release(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        *self
            .on_last_release
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(notify);
    }

    /// Drops only the nested `"sleep-inhibit"` lease.
    ///
    /// Other SoftSuspend holders (`wifi`, `input`, …) keep their own leases.
    /// Named Sleep holders stay alive; [`Self::resume`] re-takes this lease if
    /// any remain.
    #[cfg(any(feature = "kobo", docsrs))]
    fn pause(&self) {
        let mut wake = self.wake.lock().unwrap_or_else(|e| e.into_inner());
        wake.paused = true;
        wake.lease = None;
    }

    /// Clears the pause flag and re-takes `"sleep-inhibit"` when Sleep holders remain.
    ///
    /// Does not touch other SoftSuspend leases. If `holders_alive` is false,
    /// only the pause flag is cleared so a later first acquire can take the lock.
    #[cfg(any(feature = "kobo", docsrs))]
    fn resume(&self, holders_alive: bool) {
        let mut wake = self.wake.lock().unwrap_or_else(|e| e.into_inner());
        wake.paused = false;
        if holders_alive && wake.lease.is_none() {
            wake.lease = Some(
                self.soft_suspend
                    .acquire_lease(LeaseName::new(SLEEP_INHIBIT_NAME)),
            );
        }
    }
}

impl LeaseObserver for SleepInhibitResources {
    fn on_first_acquire(&self, _name: &LeaseName) {
        let mut wake = self.wake.lock().unwrap_or_else(|e| e.into_inner());
        if wake.paused {
            return;
        }
        wake.lease = Some(
            self.soft_suspend
                .acquire_lease(LeaseName::new(SLEEP_INHIBIT_NAME)),
        );
    }

    fn on_last_release(&self, _name: &LeaseName) {
        self.wake.lock().unwrap_or_else(|e| e.into_inner()).lease = None;
        if let Some(notify) = self
            .on_last_release
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            notify();
        }
    }
}

/// Named Sleep-holder tracker plus optional handle for the last-release
/// callback.
///
/// [`Self::acquire`] returns a [`Lease`] wrapped by
/// [`InhibitorGuard`](super::InhibitorGuard). Dropping the last lease runs
/// [`LeaseObserver::on_last_release`] and the last-release notifier.
pub(super) struct SleepInhibitState {
    tracker: LeaseTracker,
    #[cfg(any(test, feature = "kobo", docsrs))]
    resources: Arc<SleepInhibitResources>,
}

impl SleepInhibitState {
    /// Builds a tracker whose observer drives the nested wake lock on first/last.
    pub(super) fn new(soft_suspend: Arc<dyn SoftSuspendKind>) -> Self {
        let resources = SleepInhibitResources::new(soft_suspend);
        let tracker = LeaseTracker::with_observer(Arc::clone(&resources) as Arc<dyn LeaseObserver>);
        #[cfg(any(test, feature = "kobo", docsrs))]
        {
            Self { tracker, resources }
        }
        #[cfg(not(any(test, feature = "kobo", docsrs)))]
        {
            drop(resources);
            Self { tracker }
        }
    }

    /// Installs the callback invoked when the last Sleep holder drops.
    #[cfg(any(test, feature = "kobo", docsrs))]
    pub(super) fn set_on_last_release(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        self.resources.set_on_last_release(notify);
    }

    /// Adds a named Sleep holder. Nested wake lock starts on 0→1 only.
    pub(super) fn acquire(&self, name: LeaseName) -> Lease {
        self.tracker.acquire(name)
    }

    /// Whether any named Sleep holder is still alive.
    #[cfg(any(test, feature = "kobo", docsrs))]
    pub(super) fn sleep_active(&self) -> bool {
        !self.tracker.is_empty()
    }

    /// Drops the nested `"sleep-inhibit"` wake lock without releasing Sleep holders.
    ///
    /// Explicit suspend calls this so DeepIdle can drop its cycle lease as the
    /// last *Sleep* SoftSuspend holder. Other SoftSuspend names (`wifi`,
    /// `input`, …) stay held. Named Sleep leases stay alive.
    #[cfg(any(feature = "kobo", docsrs))]
    pub(super) fn pause_wake_lock(&self) {
        self.resources.pause();
    }

    /// Re-takes `"sleep-inhibit"` if any Sleep holder is still alive.
    #[cfg(any(feature = "kobo", docsrs))]
    pub(super) fn resume_wake_lock(&self) {
        self.resources.resume(!self.tracker.is_empty());
    }
}
