//! Shared soft-suspend operations.
//!
//! The Linux session and [`NoOpSoftSuspend`](super::noop::NoOpSoftSuspend)
//! implement this contract. [`SoftSuspend`](super::SoftSuspend) implements it
//! by matching on its variants; the trait is not object-safe — `acquire` is on
//! the input path and must stay inlineable.

use super::lease::SoftSuspendLease;
use super::mode::AutosleepMode;
use crate::lease::LeaseName;
use std::time::Duration;

/// Operations every soft-suspend backend must provide.
pub trait SoftSuspendBackend: Send + Sync {
    /// Returns whether this backend can arm kernel autosleep.
    fn is_supported(&self) -> bool;

    /// Acquires a named lease.
    #[must_use = "lease is released immediately if unused; bind it (e.g. `let _lease = …`)"]
    fn acquire(&self, name: impl Into<LeaseName>) -> SoftSuspendLease;

    /// Runs `f` while holding a named lease.
    fn with<R>(&self, name: impl Into<LeaseName>, f: impl FnOnce() -> R) -> R {
        let _lease = self.acquire(name);
        f()
    }

    /// Returns current lease holder count.
    fn len(&self) -> usize;

    /// Returns whether any leases are held.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns whether any leases are held.
    fn has_holders(&self) -> bool {
        !self.is_empty()
    }

    /// Returns active lease holder names.
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
