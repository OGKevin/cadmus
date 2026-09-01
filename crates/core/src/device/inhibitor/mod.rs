//! Cadmus inhibitor subsystem.
//!
//! Named **inhibitor leases** let features tell Cadmus (and, on Linux, the
//! kernel) that critical or in-flight work must not be interrupted. Two kinds
//! exist:
//!
//! | Kind | Wake lock | Blocks Cadmus suspend | Blocks user exits |
//! |------|-----------|----------------------|-------------------|
//! | [`Kind::SoftSuspend`] | Yes (Linux) | No | No |
//! | [`Kind::Full`] | Yes (implies SoftSuspend) | Yes | Yes |
//!
//! # API ownership
//!
//! All inhibit acquires go through [`Inhibitor::acquire`]. SoftSuspend is not a
//! parallel public lease API — call sites that need SoftSuspend wake-lock
//! behaviour acquire through [`Inhibitor::acquire`] and handle [`InhibitorError`].
//!
//! # Composition
//!
//! [`Inhibitor`] orchestrates kinds. Platform SoftSuspend implementations are
//! injected as [`SoftSuspendKind`]. Linux wake
//! lock and autosleep live under [`crate::device::linux::soft_suspend`] and are
//! built by device probe code, then passed into [`Inhibitor::new`].
//! Full acquire reads capacity from the shared [`Battery`] and compares it to
//! [`FULL_INHIBIT_MIN_CAPACITY_PERCENT`].
//!
//! [`Kind::Full`] tracking, the nested `"full-inhibit"` wake lock, and the
//! status-LED pulse live in the `full` implementation module. On Kobo the last
//! Full drop posts [`Event::FullInhibitCleared`](crate::view::Event::FullInhibitCleared)
//! so the suspend orchestrator can flush a deferred explicit suspend.
//!
//! ```ignore
//! match inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Wifi) {
//!     Ok(guard) => { /* … work … */ }
//!     Err(error) => tracing::error!(error = %error, "wifi soft-suspend acquire failed"),
//! }
//! ```

mod error;
mod full;
mod guard;
mod kind;
pub(crate) mod soft_suspend;

pub use error::InhibitorError;
pub use guard::InhibitorGuard;
pub use kind::Kind;
pub use soft_suspend::SoftSuspendName;

use crate::device::battery::{Battery, FakeBattery};
use crate::device::leds::StatusLed;
use crate::device::soft_suspend::SoftSuspendBackend;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::lease::LeaseName;
use soft_suspend::{NoOpSoftSuspendKind, SoftSuspendKind};
use std::sync::Arc;
use std::time::Duration;

#[cfg(any(target_os = "linux", docsrs))]
use crate::device::leds::DeviceLeds;
#[cfg(any(target_os = "linux", docsrs))]
use crate::device::linux::soft_suspend::kind::LinuxSoftSuspendKind;
#[cfg(all(test, target_os = "linux"))]
use crate::device::linux::soft_suspend::paths::SoftSuspendPaths;

/// Minimum battery capacity (percent) required to acquire [`Kind::Full`].
///
/// Matches Kobo firmware: Nickel refuses to install `KoboRoot.tgz` (and similar
/// onboard updates) when capacity is below 20%. Full inhibit is used for work
/// that ends in such an update path (for example OTA), so Cadmus uses the same
/// floor rather than [`BatterySettings::power_off`](crate::settings::BatterySettings::power_off).
pub(crate) const FULL_INHIBIT_MIN_CAPACITY_PERCENT: f32 = 20.0;

/// Top-level inhibitor: Kind orchestration over injected backends.
///
/// Construct with [`Self::new`] after building a SoftSuspend-kind backend, or
/// [`Self::noop`] for emulator / unsupported hosts. Store on
/// [`Context`](crate::context::Context) and acquire via [`Self::acquire`].
///
/// Implements [`SoftSuspendBackend`] by forwarding settings and holder queries
/// to the injected SoftSuspend kind. Full-holder liveness is a separate query
/// (`full_active`, Kobo / tests), not this trait.
pub struct Inhibitor {
    soft_suspend: Arc<dyn SoftSuspendKind>,
    full: full::FullInhibitState,
    battery: Arc<dyn Battery>,
}

impl Inhibitor {
    /// Composes an inhibitor from SoftSuspend backend, LED arbiter, and shared battery.
    ///
    /// The same [`SoftSuspendKind`] is used for direct SoftSuspend acquires and
    /// for the nested `"full-inhibit"` lease taken while any Full holder is
    /// active. [`StatusLed`] multiplexes Full pulse over soft-indicate.
    pub fn new(
        soft_suspend: Arc<dyn SoftSuspendKind>,
        status_led: Arc<StatusLed>,
        battery: Arc<dyn Battery>,
    ) -> Arc<Self> {
        let full = full::FullInhibitState::new(Arc::clone(&soft_suspend), status_led);
        Arc::new(Self {
            soft_suspend,
            full,
            battery,
        })
    }

    /// Inert inhibitor: NoOp SoftSuspend kind, no LED hardware, shared fake battery.
    pub fn noop() -> Arc<Self> {
        Self::new(
            NoOpSoftSuspendKind::new(),
            StatusLed::new(None),
            Arc::new(FakeBattery::new()),
        )
    }

    /// NoOp SoftSuspend with an injected shared battery (tests / emulator wiring).
    ///
    /// Compiled for emulator, standalone deviceless (`TestDevice` when `kobo`
    /// and `emulator` are off), tests, and rustdoc. A `--workspace` kobo
    /// clippy build still unifies `deviceless` from importer/fetcher; the
    /// extra `not(kobo)` arm keeps this helper off that lib target so it is
    /// not unused.
    #[cfg(any(
        test,
        docsrs,
        feature = "emulator",
        all(
            feature = "deviceless",
            not(any(feature = "kobo", feature = "emulator"))
        )
    ))]
    pub(crate) fn noop_with_battery(battery: Arc<dyn Battery>) -> Arc<Self> {
        Self::new(NoOpSoftSuspendKind::new(), StatusLed::new(None), battery)
    }

    /// Builds a Linux SoftSuspend kind when sysfs is available, otherwise an
    /// inert NoOp that never touches `/sys/power`.
    #[cfg(any(target_os = "linux", docsrs))]
    pub fn from_system(leds: Option<Arc<dyn DeviceLeds>>, battery: Arc<dyn Battery>) -> Arc<Self> {
        let status_led = StatusLed::new(leds);
        match LinuxSoftSuspendKind::try_from_system(Arc::clone(&status_led)) {
            Some(kind) => Self::new(kind, status_led, battery),
            None => Self::new(NoOpSoftSuspendKind::new(), status_led, battery),
        }
    }

    /// Always constructs a live Linux SoftSuspend inhibitor with injectable paths (tests).
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn with_paths(
        paths: SoftSuspendPaths,
        leds: Option<Arc<dyn DeviceLeds>>,
        battery: Arc<dyn Battery>,
    ) -> Arc<Self> {
        let status_led = StatusLed::new(leds);
        let kind = LinuxSoftSuspendKind::with_paths(paths, Arc::clone(&status_led));
        Self::new(kind, status_led, battery)
    }

    /// Registers a callback invoked on the thread that drops the last Full holder.
    ///
    /// Kobo lifecycle posts [`Event::FullInhibitCleared`](crate::view::Event::FullInhibitCleared)
    /// so [`start_cycle`](crate::device::suspend::start_cycle) can run if
    /// [`Context::deferred_suspend`](crate::context::Context::deferred_suspend)
    /// is set. The callback is **not** the UI thread; it only enqueues.
    ///
    /// Replacing the notifier drops the previous callback. OTA success should
    /// send [`Event::ClearDeferredSuspend`](crate::view::Event::ClearDeferredSuspend)
    /// *before* dropping `"ota"` so this flush is a no-op across reboot.
    #[cfg(any(test, feature = "kobo", docsrs))]
    pub(crate) fn set_full_release_notifier(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        self.full.set_on_last_release(notify);
    }

    /// Returns whether any [`Kind::Full`] holder is active.
    ///
    /// Kobo uses this to defer [`start_cycle`](crate::device::suspend::start_cycle)
    /// and to ignore user exits (menu power-off / restart / quit, long-press
    /// power, live AutoPowerOff). SoftSuspend-only holders do not count.
    #[cfg(any(test, feature = "kobo", docsrs))]
    pub(crate) fn full_active(&self) -> bool {
        self.full.full_active()
    }

    /// Acquires a named inhibitor guard.
    ///
    /// [`Kind::SoftSuspend`] maps to the injected SoftSuspend-kind wake lock and
    /// always succeeds on supported backends. [`Kind::Full`] also registers a
    /// Full holder, nested `"full-inhibit"` wake lock, and status-LED blink.
    ///
    /// Drop the returned [`InhibitorGuard`] to release. The last Full drop
    /// fires the last-release notifier if one is installed (Kobo posts
    /// [`Event::FullInhibitCleared`](crate::view::Event::FullInhibitCleared)).
    ///
    /// # Errors
    ///
    /// [`Kind::Full`] fails with [`InhibitorError::BatteryTooLow`] when shared
    /// battery capacity is below [`FULL_INHIBIT_MIN_CAPACITY_PERCENT`] or
    /// cannot be read. [`Kind::SoftSuspend`] is never battery-gated.
    pub fn acquire(
        &self,
        kind: Kind,
        name: impl Into<LeaseName>,
    ) -> Result<InhibitorGuard, InhibitorError> {
        let name = name.into();
        match kind {
            Kind::SoftSuspend => Ok(InhibitorGuard::soft_suspend(
                self.soft_suspend.acquire_lease(name),
            )),
            Kind::Full => {
                self.ensure_battery_allows_full()?;
                let lease = self.full.acquire(name);
                Ok(InhibitorGuard::full(lease))
            }
        }
    }

    fn ensure_battery_allows_full(&self) -> Result<(), InhibitorError> {
        let capacity = self
            .battery
            .capacity()
            .ok()
            .and_then(|values| values.into_iter().next())
            .ok_or(InhibitorError::BatteryTooLow)?;
        if capacity < FULL_INHIBIT_MIN_CAPACITY_PERCENT {
            return Err(InhibitorError::BatteryTooLow);
        }
        Ok(())
    }
}

impl SoftSuspendBackend for Inhibitor {
    fn is_supported(&self) -> bool {
        self.soft_suspend.is_supported()
    }

    fn mode(&self) -> AutosleepMode {
        self.soft_suspend.mode()
    }

    fn indicate_autosleep_led(&self) -> bool {
        self.soft_suspend.indicate_autosleep_led()
    }

    fn autosleep_grace(&self) -> Duration {
        self.soft_suspend.autosleep_grace()
    }

    fn available_modes(&self) -> Vec<AutosleepMode> {
        self.soft_suspend.available_modes()
    }

    fn sanitize_mode(&self, mode: AutosleepMode) -> AutosleepMode {
        self.soft_suspend.sanitize_mode(mode)
    }

    fn set_mode(&self, mode: AutosleepMode) {
        self.soft_suspend.set_mode(mode);
    }

    fn set_indicate_autosleep_led(&self, enabled: bool) {
        self.soft_suspend.set_indicate_autosleep_led(enabled);
    }

    fn set_autosleep_grace(&self, grace: Duration) {
        self.soft_suspend.set_autosleep_grace(grace);
    }

    fn len(&self) -> usize {
        self.soft_suspend.len()
    }

    fn holders(&self) -> Vec<LeaseName> {
        self.soft_suspend.holders()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn inhibitor_with_capacity(capacity: f32) -> Arc<Inhibitor> {
        let battery = Arc::new(FakeBattery::new());
        battery.set_capacity(capacity);
        Inhibitor::noop_with_battery(battery)
    }

    #[test]
    fn soft_suspend_acquire_on_noop() {
        let inhibitor = Inhibitor::noop();
        let guard = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Wifi)
            .unwrap();
        assert!(guard.is_active());
        assert_eq!(inhibitor.len(), 1);
        drop(guard);
        assert_eq!(inhibitor.len(), 0);
    }

    #[test]
    fn full_tracking_on_noop() {
        let inhibitor = inhibitor_with_capacity(50.0);
        let guard = inhibitor.acquire(Kind::Full, "ota").unwrap();
        assert!(inhibitor.full_active());
        drop(guard);
        assert!(!inhibitor.full_active());
    }

    #[test]
    fn full_implies_nested_soft_suspend_wake_lock() {
        let inhibitor = inhibitor_with_capacity(50.0);
        let _guard = inhibitor.acquire(Kind::Full, "ota").unwrap();
        assert!(
            inhibitor
                .holders()
                .iter()
                .any(|name| name.as_str() == "full-inhibit")
        );
    }

    #[test]
    fn soft_suspend_only_does_not_block_full() {
        let inhibitor = Inhibitor::noop();
        let _soft = inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Wifi)
            .unwrap();
        assert!(!inhibitor.full_active());
    }

    #[test]
    fn full_acquire_rejected_below_min_capacity() {
        let inhibitor = inhibitor_with_capacity(FULL_INHIBIT_MIN_CAPACITY_PERCENT - 1.0);
        let result = inhibitor.acquire(Kind::Full, "ota");
        assert!(matches!(result, Err(InhibitorError::BatteryTooLow)));
        assert!(!inhibitor.full_active());
    }

    #[test]
    fn full_acquire_allowed_at_min_capacity() {
        let inhibitor = inhibitor_with_capacity(FULL_INHIBIT_MIN_CAPACITY_PERCENT);
        assert!(inhibitor.acquire(Kind::Full, "ota").is_ok());
    }

    #[test]
    fn full_release_notifier_runs_on_last_drop() {
        let inhibitor = inhibitor_with_capacity(50.0);
        let fired = Arc::new(AtomicU32::new(0));
        let fired_cb = Arc::clone(&fired);
        inhibitor.set_full_release_notifier(Arc::new(move || {
            fired_cb.fetch_add(1, Ordering::SeqCst);
        }));
        let guard = inhibitor.acquire(Kind::Full, "ota").unwrap();
        assert_eq!(fired.load(Ordering::SeqCst), 0);
        drop(guard);
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }
}
