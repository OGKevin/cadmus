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
//! other SoftSuspend lease is held.
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

/// Shared side-effect state for the Sleep 0→1 / 1→0 transitions.
struct SleepInhibitResources {
    soft_suspend: Arc<dyn SoftSuspendKind>,
    /// Nested `"sleep-inhibit"` SoftSuspend lease; `Some` iff Sleep holders > 0.
    wake_lease: Mutex<Option<Lease>>,
}

impl SleepInhibitResources {
    fn new(soft_suspend: Arc<dyn SoftSuspendKind>) -> Arc<Self> {
        Arc::new(Self {
            soft_suspend,
            wake_lease: Mutex::new(None),
        })
    }
}

impl LeaseObserver for SleepInhibitResources {
    fn on_first_acquire(&self, _name: &LeaseName) {
        let wake = self
            .soft_suspend
            .acquire_lease(LeaseName::new(SLEEP_INHIBIT_NAME));
        *self.wake_lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
    }

    fn on_last_release(&self, _name: &LeaseName) {
        *self.wake_lease.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Named Sleep-holder tracker.
///
/// [`Self::acquire`] returns a [`Lease`] wrapped by
/// [`InhibitorGuard`](super::InhibitorGuard). Dropping the last lease runs
/// [`LeaseObserver::on_last_release`].
pub(super) struct SleepInhibitState {
    tracker: LeaseTracker,
}

impl SleepInhibitState {
    /// Builds a tracker whose observer drives the nested wake lock on first/last.
    pub(super) fn new(soft_suspend: Arc<dyn SoftSuspendKind>) -> Self {
        let tracker = LeaseTracker::with_observer(SleepInhibitResources::new(soft_suspend));
        Self { tracker }
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
}
