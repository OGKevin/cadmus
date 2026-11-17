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
//! other SoftSuspend lease is held. The suspend orchestrator releases the
//! shared `cadmus` lock for the kernel-sleep window without dropping this
//! nested lease; restoring the cycle re-acquires it while holders remain.
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

/// Shared side-effect state for the Sleep 0→1 / 1→0 transitions.
///
/// Held by the [`LeaseObserver`] and, on kobo/tests/docsrs, by
/// [`SleepInhibitState`] so the last-release notifier can be installed after
/// construction.
struct SleepInhibitResources {
    soft_suspend: Arc<dyn SoftSuspendKind>,
    wake_lease: Mutex<Option<Lease>>,
    /// Optional callback after the last Sleep holder drops (Kobo posts
    /// [`Event::InhibitCleared`](crate::view::Event::InhibitCleared) with
    /// [`Kind::Sleep`](super::Kind::Sleep)).
    on_last_release: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl SleepInhibitResources {
    fn new(soft_suspend: Arc<dyn SoftSuspendKind>) -> Arc<Self> {
        Arc::new(Self {
            soft_suspend,
            wake_lease: Mutex::new(None),
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
}

impl LeaseObserver for SleepInhibitResources {
    fn on_first_acquire(&self, _name: &LeaseName) {
        *self.wake_lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(
            self.soft_suspend
                .acquire_lease(LeaseName::new(SLEEP_INHIBIT_NAME)),
        );
    }

    fn on_last_release(&self, _name: &LeaseName) {
        *self.wake_lease.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
}
