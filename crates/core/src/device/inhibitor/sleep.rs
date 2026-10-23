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
//!                              │              └─ drop wake lease
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
//! Unlike [`Kind::Full`](super::Kind::Full), Sleep has no battery gate, no
//! status LED, and no last-release notifier. Last drop does **not** post
//! [`Event::FullInhibitCleared`](crate::view::Event::FullInhibitCleared).

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
struct SleepInhibitResources {
    soft_suspend: Arc<dyn SoftSuspendKind>,
    wake: Mutex<SleepWake>,
}

impl SleepInhibitResources {
    fn new(soft_suspend: Arc<dyn SoftSuspendKind>) -> Arc<Self> {
        Arc::new(Self {
            soft_suspend,
            wake: Mutex::new(SleepWake {
                lease: None,
                paused: false,
            }),
        })
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
    }
}

/// Named Sleep-holder tracker.
///
/// [`Self::acquire`] returns a [`Lease`] wrapped by
/// [`InhibitorGuard`](super::InhibitorGuard). Dropping the last lease runs
/// [`LeaseObserver::on_last_release`].
pub(super) struct SleepInhibitState {
    tracker: LeaseTracker,
    #[cfg(any(feature = "kobo", docsrs))]
    resources: Arc<SleepInhibitResources>,
}

impl SleepInhibitState {
    /// Builds a tracker whose observer drives the nested wake lock on first/last.
    pub(super) fn new(soft_suspend: Arc<dyn SoftSuspendKind>) -> Self {
        let resources = SleepInhibitResources::new(soft_suspend);
        let tracker = LeaseTracker::with_observer(Arc::clone(&resources) as Arc<dyn LeaseObserver>);
        #[cfg(any(feature = "kobo", docsrs))]
        {
            Self { tracker, resources }
        }
        #[cfg(not(any(feature = "kobo", docsrs)))]
        {
            drop(resources);
            Self { tracker }
        }
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
