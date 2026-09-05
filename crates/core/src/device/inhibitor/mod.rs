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
//! behaviour use `inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::…)`.
//!
//! # Composition
//!
//! [`Inhibitor`] orchestrates kinds. Platform SoftSuspend implementations are
//! injected as [`SoftSuspendKind`]. Linux wake
//! lock and autosleep live under [`crate::device::linux::soft_suspend`] and are
//! built by device probe code, then passed into [`Inhibitor::new`].
//!
//! ```ignore
//! let guard = inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Wifi);
//! // … work …
//! drop(guard);
//! ```

mod error;
mod guard;
mod kind;
pub(crate) mod soft_suspend;

pub use error::InhibitorError;
pub use guard::InhibitorGuard;
pub use kind::Kind;
pub use soft_suspend::SoftSuspendName;

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
#[cfg(any(test, docsrs))]
#[cfg(any(target_os = "linux", docsrs))]
use crate::device::linux::soft_suspend::paths::SoftSuspendPaths;

/// Top-level inhibitor: Kind orchestration over injected backends.
///
/// Construct with [`Self::new`] after building a SoftSuspend-kind backend, or
/// [`Self::noop`] for emulator / unsupported hosts. Store on
/// [`Context`](crate::context::Context) and acquire via [`Self::acquire`].
pub struct Inhibitor {
    soft_suspend: Arc<dyn SoftSuspendKind>,
    #[expect(
        dead_code,
        reason = "Full inhibit will drive the status LED from Inhibitor::acquire"
    )]
    status_led: Arc<StatusLed>,
}

impl Inhibitor {
    /// Composes an inhibitor from an injected SoftSuspend-kind backend and LED arbiter.
    pub fn new(soft_suspend: Arc<dyn SoftSuspendKind>, status_led: Arc<StatusLed>) -> Arc<Self> {
        Arc::new(Self {
            soft_suspend,
            status_led,
        })
    }

    /// Inert inhibitor: NoOp SoftSuspend kind and no LED hardware.
    pub fn noop() -> Arc<Self> {
        Self::new(NoOpSoftSuspendKind::new(), StatusLed::new(None))
    }

    /// Builds a Linux SoftSuspend kind when sysfs is available, otherwise an
    /// inert NoOp that never touches `/sys/power`.
    #[cfg(any(target_os = "linux", docsrs))]
    pub fn from_system(leds: Option<Arc<dyn DeviceLeds>>) -> Arc<Self> {
        let status_led = StatusLed::new(leds);
        match LinuxSoftSuspendKind::try_from_system(Arc::clone(&status_led)) {
            Some(kind) => Self::new(kind, status_led),
            None => Self::new(NoOpSoftSuspendKind::new(), status_led),
        }
    }

    /// Always constructs a live Linux SoftSuspend inhibitor with injectable paths (tests).
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn with_paths(
        paths: SoftSuspendPaths,
        leds: Option<Arc<dyn DeviceLeds>>,
    ) -> Arc<Self> {
        let status_led = StatusLed::new(leds);
        let kind = LinuxSoftSuspendKind::with_paths(paths, Arc::clone(&status_led));
        Self::new(kind, status_led)
    }

    /// Acquires a named inhibitor guard.
    ///
    /// [`Kind::SoftSuspend`] maps to the injected SoftSuspend-kind wake lock.
    /// [`Kind::Full`] panics until Full inhibit is implemented.
    pub fn acquire(&self, kind: Kind, name: impl Into<LeaseName>) -> InhibitorGuard {
        let name = name.into();
        match kind {
            Kind::SoftSuspend => {
                InhibitorGuard::soft_suspend(self.soft_suspend.acquire_lease(name))
            }
            Kind::Full => unimplemented!("Full inhibit"),
        }
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

    #[test]
    #[should_panic = "Full inhibit"]
    fn full_inhibitor_unavailable() {
        let inhibitor = Inhibitor::noop();
        let _guard = inhibitor.acquire(Kind::Full, "ota");
    }

    #[test]
    fn soft_suspend_acquire_on_noop() {
        let inhibitor = Inhibitor::noop();
        let guard = inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Wifi);
        assert!(guard.is_active());
        assert_eq!(inhibitor.len(), 1);
        drop(guard);
        assert_eq!(inhibitor.len(), 0);
    }
}
