//! No-op soft-suspend backend for emulator, tests, and unsupported hosts.

use super::SoftSuspendBackend;
use super::lease::SoftSuspendLease;
use super::mode::AutosleepMode;
use crate::lease::LeaseName;
use std::time::Duration;

/// Inert soft-suspend backend: unarmed, empty leases, no sysfs, no unlock worker.
///
/// Implements [`super::SoftSuspendBackend`].
#[derive(Debug, Default)]
pub struct NoOpSoftSuspend;

impl SoftSuspendBackend for NoOpSoftSuspend {
    fn is_supported(&self) -> bool {
        false
    }

    fn acquire(&self, _name: impl Into<LeaseName>) -> SoftSuspendLease {
        SoftSuspendLease::noop()
    }

    fn len(&self) -> usize {
        0
    }

    fn holders(&self) -> Vec<LeaseName> {
        Vec::new()
    }

    fn mode(&self) -> AutosleepMode {
        AutosleepMode::Off
    }

    fn indicate_autosleep_led(&self) -> bool {
        false
    }

    fn autosleep_grace(&self) -> Duration {
        Duration::ZERO
    }

    fn available_modes(&self) -> Vec<AutosleepMode> {
        vec![AutosleepMode::Off]
    }

    fn sanitize_mode(&self, _mode: AutosleepMode) -> AutosleepMode {
        AutosleepMode::Off
    }

    fn set_mode(&self, _mode: AutosleepMode) {}

    fn set_indicate_autosleep_led(&self, _enabled: bool) {}

    fn set_autosleep_grace(&self, _grace: Duration) {}
}
