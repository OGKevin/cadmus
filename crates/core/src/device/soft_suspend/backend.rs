//! Soft-suspend settings contract.
//!
//! Implemented by the Linux SoftSuspend kind, the NoOp kind, and
//! [`Inhibitor`](crate::device::inhibitor::Inhibitor). Covers autosleep mode,
//! LED preference, release grace, and holder introspection. Lease acquire is
//! **not** on this trait — use
//! [`Inhibitor::acquire`](crate::device::inhibitor::Inhibitor::acquire).

use super::mode::AutosleepMode;
use crate::lease::LeaseName;
use std::time::Duration;

/// Settings and diagnostics for SoftSuspend behaviour.
///
/// Live Linux backends report [`Self::is_supported`] as `true` and write sysfs;
/// noop backends accept calls without effect. Settings UI and the suspend
/// orchestrator use this contract via [`Inhibitor`](crate::device::inhibitor::Inhibitor).
pub trait SoftSuspendBackend: Send + Sync {
    /// Returns whether this backend can arm kernel autosleep.
    fn is_supported(&self) -> bool;

    /// Returns current SoftSuspend lease holder count.
    fn len(&self) -> usize;

    /// Returns whether no SoftSuspend leases are held.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns whether any SoftSuspend leases are held.
    fn has_holders(&self) -> bool {
        !self.is_empty()
    }

    /// Returns active SoftSuspend lease holder names.
    fn holders(&self) -> Vec<LeaseName>;

    /// Returns the current autosleep mode.
    fn mode(&self) -> AutosleepMode;

    /// Returns whether the status LED indicates armed soft suspend while awake.
    fn indicate_autosleep_led(&self) -> bool;

    /// Returns the delay after the last lease drops before writing `wake_unlock`.
    fn autosleep_grace(&self) -> Duration;

    /// Modes supported by this backend (`Off` plus discovery on Linux).
    fn available_modes(&self) -> Vec<AutosleepMode>;

    /// Sanitizes `mode` against discovery.
    fn sanitize_mode(&self, mode: AutosleepMode) -> AutosleepMode;

    /// Sets autosleep mode.
    fn set_mode(&self, mode: AutosleepMode);

    /// Enables or disables the status LED while soft suspend is armed.
    fn set_indicate_autosleep_led(&self, enabled: bool);

    /// Sets how long to keep the wake lock after the last lease drops.
    fn set_autosleep_grace(&self, grace: Duration);

    /// Applies mode, LED policy, and release grace from settings.
    fn apply_settings(
        &self,
        mode: AutosleepMode,
        indicate_autosleep_led: bool,
        autosleep_grace: Duration,
    ) {
        self.set_autosleep_grace(autosleep_grace);
        self.set_indicate_autosleep_led(indicate_autosleep_led);
        self.set_mode(mode);
    }
}
