//! No-op SoftSuspend-kind backend.
//!
//! Tracks holders in-process without touching sysfs or running an unlock worker.
//! Used on the emulator, in unit tests, and when the Linux power sysfs probe
//! fails.

use super::SoftSuspendKind;
use crate::device::soft_suspend::SoftSuspendBackend;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::lease::{Lease, LeaseName, LeaseTracker};
use std::sync::Arc;
use std::time::Duration;

/// Inert SoftSuspend-kind: in-process lease tracking, no sysfs, no unlock worker.
pub(crate) struct NoOpSoftSuspendKind {
    tracker: LeaseTracker,
}

impl NoOpSoftSuspendKind {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            tracker: LeaseTracker::new(),
        })
    }
}

impl SoftSuspendBackend for NoOpSoftSuspendKind {
    fn is_supported(&self) -> bool {
        false
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

    fn len(&self) -> usize {
        self.tracker.len()
    }

    fn holders(&self) -> Vec<LeaseName> {
        self.tracker.holders()
    }
}

impl SoftSuspendKind for NoOpSoftSuspendKind {
    fn acquire_lease(&self, name: LeaseName) -> Lease {
        self.tracker.acquire(name)
    }
}
