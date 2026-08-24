//! Linux SoftSuspend-kind backend composed of wake lock + autosleep policy.
//!
//! Coordinates named SoftSuspend leases, autosleep mode, and optional LED
//! soft-indicate. Built by device probe code and injected into
//! [`Inhibitor`](crate::device::inhibitor::Inhibitor). Does not construct the
//! inhibitor itself.

use super::autosleep::AutosleepPolicy;
use super::paths::SoftSuspendPaths;
use super::wake::WakeLock;
use crate::device::inhibitor::soft_suspend::SoftSuspendKind;
use crate::device::leds::StatusLed;
use crate::device::soft_suspend::SoftSuspendBackend;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::lease::{Lease, LeaseName};
use std::sync::Arc;
use std::time::Duration;

/// Live SoftSuspend-kind implementation for Linux/Kobo.
///
/// Coordinates named SoftSuspend leases over one `cadmus` wake lock, autosleep
/// mode, and optional LED soft-indicate.
pub(crate) struct LinuxSoftSuspendKind {
    wake: Arc<WakeLock>,
    autosleep: AutosleepPolicy,
}

impl LinuxSoftSuspendKind {
    /// Probes system sysfs and returns a live kind, or `None` when unsupported.
    pub(crate) fn try_from_system(status_led: Arc<StatusLed>) -> Option<Arc<Self>> {
        Self::try_from_paths(SoftSuspendPaths::system(), status_led)
    }

    /// Probes `paths` and returns a live kind, or `None` when unsupported.
    pub(crate) fn try_from_paths(
        paths: SoftSuspendPaths,
        status_led: Arc<StatusLed>,
    ) -> Option<Arc<Self>> {
        match paths.probe() {
            Ok(ok) => {
                tracing::debug!(
                    autosleep = %ok.autosleep().display(),
                    "soft-suspend kind supported"
                );
                Some(Self::new(ok.into_paths(), status_led))
            }
            Err(paths) => {
                tracing::info!(
                    autosleep = %paths.autosleep.display(),
                    wake_lock = %paths.wake_lock.display(),
                    "soft-suspend kind unsupported"
                );
                None
            }
        }
    }

    /// Always constructs a live kind with injectable paths (tests).
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn with_paths(paths: SoftSuspendPaths, status_led: Arc<StatusLed>) -> Arc<Self> {
        let paths = super::paths::SoftSuspendProbeOk::assume(paths).into_paths();
        Self::new(paths, status_led)
    }

    fn new(paths: SoftSuspendPaths, status_led: Arc<StatusLed>) -> Arc<Self> {
        let wake = WakeLock::new(paths.clone());
        Arc::new(Self {
            wake: Arc::clone(&wake),
            autosleep: AutosleepPolicy::new(paths, wake, status_led),
        })
    }
}

impl SoftSuspendBackend for LinuxSoftSuspendKind {
    fn is_supported(&self) -> bool {
        true
    }

    fn mode(&self) -> AutosleepMode {
        self.autosleep.mode()
    }

    fn indicate_autosleep_led(&self) -> bool {
        self.autosleep.indicate_autosleep_led()
    }

    fn autosleep_grace(&self) -> Duration {
        self.autosleep.autosleep_grace()
    }

    fn available_modes(&self) -> Vec<AutosleepMode> {
        self.autosleep.available_modes()
    }

    fn sanitize_mode(&self, mode: AutosleepMode) -> AutosleepMode {
        self.autosleep.sanitize_mode(mode)
    }

    fn set_mode(&self, mode: AutosleepMode) {
        self.autosleep.set_mode(mode);
    }

    fn set_indicate_autosleep_led(&self, enabled: bool) {
        self.autosleep.set_indicate_autosleep_led(enabled);
    }

    fn set_autosleep_grace(&self, grace: Duration) {
        self.autosleep.set_autosleep_grace(grace);
    }

    fn len(&self) -> usize {
        self.wake.len()
    }

    fn holders(&self) -> Vec<LeaseName> {
        self.wake.holders()
    }
}

impl SoftSuspendKind for LinuxSoftSuspendKind {
    fn acquire_lease(&self, name: LeaseName) -> Lease {
        self.wake.acquire(name)
    }
}
