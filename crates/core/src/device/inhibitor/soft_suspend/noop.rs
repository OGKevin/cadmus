//! No-op SoftSuspend-kind backend.
//!
//! Tracks holders in-process without touching sysfs or running an unlock worker.
//! Used on the emulator, in unit tests, and when the Linux power sysfs probe
//! fails.

use super::{SoftSuspendKind, SuppressionEpoch, WakeLockPauseOutcome, WakeLockResumeOutcome};
use crate::device::soft_suspend::SoftSuspendBackend;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::lease::{Lease, LeaseName, LeaseTracker, WeakLeaseTracker};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Inert SoftSuspend-kind: in-process lease tracking, no sysfs, no unlock worker.
pub(crate) struct NoOpSoftSuspendKind {
    /// Tracks simulated SoftSuspend wake-lock holders.
    tracker: LeaseTracker,
    /// Watches Full holders so suppression follows the real backend's gate.
    full_tracker: OnceLock<WeakLeaseTracker>,
    /// Identifies the simulated suppression window; zero marks it ended.
    suppress_epoch: AtomicU64,
}

impl NoOpSoftSuspendKind {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            tracker: LeaseTracker::new(),
            full_tracker: OnceLock::new(),
            suppress_epoch: AtomicU64::new(0),
        })
    }
}

#[async_trait::async_trait]
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

    async fn set_mode(&self, _mode: AutosleepMode) -> Result<(), std::io::Error> {
        Ok(())
    }

    fn set_indicate_autosleep_led(&self, _enabled: bool) {}

    fn set_autosleep_grace(&self, _grace: Duration) {}

    fn len(&self) -> usize {
        self.tracker.len()
    }

    fn holders(&self) -> Vec<LeaseName> {
        self.tracker.holders()
    }
}

#[async_trait::async_trait]
impl SoftSuspendKind for NoOpSoftSuspendKind {
    fn register_lease(&self, name: LeaseName) -> Lease {
        self.tracker.acquire(name)
    }

    async fn confirm_leases(&self) -> Result<(), crate::device::inhibitor::InhibitorError> {
        Ok(())
    }

    fn set_full_tracker(&self, tracker: WeakLeaseTracker) {
        let _ = self.full_tracker.set(tracker);
    }

    async fn pause_wake_lock(&self) -> WakeLockPauseOutcome {
        if self
            .full_tracker
            .get()
            .is_some_and(|tracker| !tracker.is_empty())
        {
            return WakeLockPauseOutcome::BlockedByFull;
        }
        WakeLockPauseOutcome::Paused(SuppressionEpoch(
            self.suppress_epoch.fetch_add(1, Ordering::SeqCst) + 1,
        ))
    }

    async fn resume_wake_lock(&self) -> WakeLockResumeOutcome {
        self.suppress_epoch.store(0, Ordering::SeqCst);
        if self.tracker.is_empty() {
            return WakeLockResumeOutcome::Unheld;
        }
        WakeLockResumeOutcome::Restored
    }

    fn wake_lock_epoch_is_current(&self, epoch: SuppressionEpoch) -> bool {
        epoch.0 != 0 && self.suppress_epoch.load(Ordering::SeqCst) == epoch.0
    }
}
