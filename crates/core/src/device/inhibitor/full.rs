//! Full-inhibit holder tracking, nested wake lock, and status-LED pulse.
//!
//! [`Kind::Full`](super::Kind::Full) is a **named holder set** plus two side
//! effects that apply only while the set is non-empty. Callers still acquire
//! through [`Inhibitor::acquire`](super::Inhibitor::acquire); this module owns
//! the tracker and the first/last-holder transitions.
//!
//! # Architecture
//!
//! ```text
//!   acquire("ota") ──► LeaseTracker ──► FullInhibitObserver
//!                          │                    │
//!                          │         0 → 1  on_first_full_holder
//!                          │              ├─ SoftSuspend lease "full-inhibit"
//!                          │              └─ StatusLed pulse (FullInhibit)
//!                          │         1 → 0  on_last_full_holder
//!                          │              ├─ drop wake lease + LED guard
//!                          │              └─ on_last_release notifier
//!                          ▼
//!                     Lease (RAII; InhibitorGuard::full wraps it)
//! ```
//!
//! Named Full holders (`"ota"`, tests, …) are **not** SoftSuspend names. The
//! nested wake lock is always [`FULL_INHIBIT_NAME`] (`"full-inhibit"`) so the
//! kernel `cadmus` lock stays taken for the whole Full window, even if no
//! other SoftSuspend lease is held.
//!
//! Battery gating happens in [`Inhibitor`](super::Inhibitor) **before**
//! [`FullInhibitState::acquire`]. This module does not read capacity.
//!
//! # Last-holder notifier
//!
//! Kobo registers a last-release notifier that posts
//! [`Event::FullInhibitCleared`](crate::view::Event::FullInhibitCleared)
//! so the suspend orchestrator can flush a deferred explicit suspend on the
//! main loop. The callback runs on the thread that drops the last guard
//! (OTA worker, test, …), not on the UI thread.
//!
//! The `resources` handle used to install that callback is compiled only for
//! `kobo`, tests, and rustdoc — emulator / standalone-deviceless lib builds
//! never wire the notifier.

use super::soft_suspend::SoftSuspendKind;
use crate::device::leds::{LedPattern, LedPriority, StatusLed, StatusLedGuard};
use crate::lease::{Lease, LeaseName, LeaseObserver, LeaseTracker};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// SoftSuspend lease name taken for the nested wake lock while any Full holder
/// is active. Distinct from caller names such as `"ota"`.
const FULL_INHIBIT_NAME: &str = "full-inhibit";

/// Status-LED on interval while Full inhibit is active.
///
/// Hardware is on/off only; this long-on / short-off pulse is easier to
/// distinguish from the solid soft-indicate LED than a 50/50 blink.
const BLINK_ON: Duration = Duration::from_millis(1600);

/// Status-LED off interval between [`BLINK_ON`] pulses.
const BLINK_OFF: Duration = Duration::from_millis(200);

/// Shared side-effect state for the Full 0→1 / 1→0 transitions.
///
/// Held by the [`LeaseObserver`] and, on kobo/tests/docsrs, by
/// [`FullInhibitState`] so the last-release notifier can be installed after
/// construction.
struct FullInhibitResources {
    soft_suspend: Arc<dyn SoftSuspendKind>,
    status_led: Arc<StatusLed>,
    /// Nested `"full-inhibit"` SoftSuspend lease; `Some` iff Full holders > 0.
    wake_lease: Mutex<Option<Lease>>,
    /// Pulse guard; dropped on last Full release so soft-indicate can return.
    led_guard: Mutex<Option<StatusLedGuard>>,
    /// Optional callback after the last Full holder drops (Kobo posts
    /// [`Event::FullInhibitCleared`](crate::view::Event::FullInhibitCleared)).
    on_last_release: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl FullInhibitResources {
    fn new(soft_suspend: Arc<dyn SoftSuspendKind>, status_led: Arc<StatusLed>) -> Arc<Self> {
        Arc::new(Self {
            soft_suspend,
            status_led,
            wake_lease: Mutex::new(None),
            led_guard: Mutex::new(None),
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

    /// First Full holder: take the nested wake lock and install the LED pulse.
    fn on_first_full_holder(&self) {
        let wake = self
            .soft_suspend
            .acquire_lease(LeaseName::new(FULL_INHIBIT_NAME));
        *self.wake_lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
        let guard = self.status_led.install(
            FULL_INHIBIT_NAME,
            LedPriority::FullInhibit,
            LedPattern::Blink {
                on: BLINK_ON,
                off: BLINK_OFF,
            },
        );
        *self.led_guard.lock().unwrap_or_else(|e| e.into_inner()) = Some(guard);
    }

    /// Last Full holder: drop wake lock and LED, then invoke the notifier.
    fn on_last_full_holder(&self) {
        *self.wake_lease.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.led_guard.lock().unwrap_or_else(|e| e.into_inner()) = None;
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

/// [`LeaseObserver`] that maps tracker 0→1 / 1→0 onto [`FullInhibitResources`].
struct FullInhibitObserver {
    resources: Arc<FullInhibitResources>,
}

impl LeaseObserver for FullInhibitObserver {
    fn on_first_acquire(&self, _name: &LeaseName) {
        self.resources.on_first_full_holder();
    }

    fn on_last_release(&self, _name: &LeaseName) {
        self.resources.on_last_full_holder();
    }
}

/// Named Full-holder tracker plus optional handle for the last-release callback.
///
/// [`Self::acquire`] returns a [`Lease`] wrapped by
/// [`InhibitorGuard`](super::InhibitorGuard). Dropping the last lease runs
/// [`FullInhibitResources::on_last_full_holder`].
pub(super) struct FullInhibitState {
    tracker: LeaseTracker,
    #[cfg(any(test, feature = "kobo", docsrs))]
    resources: Arc<FullInhibitResources>,
}

impl FullInhibitState {
    /// Builds a tracker whose observer drives wake lock and LED on first/last.
    pub(super) fn new(soft_suspend: Arc<dyn SoftSuspendKind>, status_led: Arc<StatusLed>) -> Self {
        let resources = FullInhibitResources::new(soft_suspend, status_led);
        let observer = Arc::new(FullInhibitObserver {
            resources: Arc::clone(&resources),
        });
        let tracker = LeaseTracker::with_observer(observer);
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

    /// Installs the callback invoked when the last Full holder drops.
    #[cfg(any(test, feature = "kobo", docsrs))]
    pub(super) fn set_on_last_release(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        self.resources.set_on_last_release(notify);
    }

    /// Adds a named Full holder. Nested wake lock / LED start on 0→1 only.
    pub(super) fn acquire(&self, name: LeaseName) -> Lease {
        self.tracker.acquire(name)
    }

    /// Whether any named Full holder is still alive.
    #[cfg(any(test, feature = "kobo", docsrs))]
    pub(super) fn full_active(&self) -> bool {
        !self.tracker.is_empty()
    }
}
