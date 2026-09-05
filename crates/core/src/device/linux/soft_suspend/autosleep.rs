//! Linux autosleep mode and soft-indicate LED policy.
//!
//! [`AutosleepPolicy`] writes `/sys/power/autosleep`, remembers the selected
//! [`AutosleepMode`], and drives the `soft-indicate` command on a shared
//! [`StatusLed`] while autosleep is armed and
//! the setting is enabled.
//!
//! Wake-lock leases live in [`super::wake`]; this module only coordinates mode
//! and LED indication with that backend.

use super::paths::{SoftSuspendPaths, SysfsWrite, discover_available_modes, write_sysfs};
use super::wake::WakeLock;
use crate::device::leds::{LedPattern, LedPriority, StatusLed, StatusLedGuard};
use crate::device::soft_suspend::mode::AutosleepMode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SOFT_INDICATE_NAME: &str = "soft-indicate";

struct PolicyState {
    mode: AutosleepMode,
    indicate_autosleep_led: bool,
    autosleep_grace: Duration,
}

/// Autosleep sysfs writes and soft-indicate LED policy for a live SoftSuspend kind.
pub(crate) struct AutosleepPolicy {
    paths: SoftSuspendPaths,
    wake: Arc<WakeLock>,
    status_led: Arc<StatusLed>,
    state: Arc<Mutex<PolicyState>>,
    soft_indicate: Mutex<Option<StatusLedGuard>>,
}

impl AutosleepPolicy {
    pub(crate) fn new(
        paths: SoftSuspendPaths,
        wake: Arc<WakeLock>,
        status_led: Arc<StatusLed>,
    ) -> Self {
        tracing::debug!(
            autosleep = %paths.autosleep.display(),
            "creating soft-suspend autosleep policy"
        );
        Self {
            paths,
            wake,
            status_led,
            state: Arc::new(Mutex::new(PolicyState {
                mode: AutosleepMode::Off,
                indicate_autosleep_led: false,
                autosleep_grace: Duration::ZERO,
            })),
            soft_indicate: Mutex::new(None),
        }
    }

    pub(crate) fn mode(&self) -> AutosleepMode {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).mode
    }

    pub(crate) fn indicate_autosleep_led(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .indicate_autosleep_led
    }

    pub(crate) fn autosleep_grace(&self) -> Duration {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .autosleep_grace
    }

    pub(crate) fn available_modes(&self) -> Vec<AutosleepMode> {
        discover_available_modes(&self.paths.state)
    }

    pub(crate) fn sanitize_mode(&self, mode: AutosleepMode) -> AutosleepMode {
        if mode == AutosleepMode::Off {
            return AutosleepMode::Off;
        }
        if self.available_modes().contains(&mode) {
            mode
        } else {
            tracing::warn!(mode = %mode, "autosleep mode unsupported; falling back to off");
            AutosleepMode::Off
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(mode = %mode), level = tracing::Level::TRACE)
    )]
    pub(crate) fn set_mode(&self, mode: AutosleepMode) {
        let mode = self.sanitize_mode(mode);
        let value = mode.as_sysfs();
        match write_sysfs(&self.paths.autosleep, value) {
            Ok(SysfsWrite::Written) => {
                tracing::debug!(
                    path = %self.paths.autosleep.display(),
                    value,
                    "wrote soft-suspend sysfs"
                );
            }
            Ok(SysfsWrite::Missing) => {
                tracing::debug!(
                    path = %self.paths.autosleep.display(),
                    value,
                    "soft-suspend sysfs path missing"
                );
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    mode = %mode,
                    "soft-suspend autosleep write failed; keeping previous mode"
                );
                return;
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let previous = state.mode;
        state.mode = mode;
        tracing::info!(previous = %previous, mode = %mode, "soft-suspend mode updated");
        drop(state);
        self.sync_soft_indicate();
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE)
    )]
    pub(crate) fn set_indicate_autosleep_led(&self, enabled: bool) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.indicate_autosleep_led = enabled;
            tracing::info!(
                enabled,
                mode = %state.mode,
                "soft-suspend LED indicator updated"
            );
        }
        self.sync_soft_indicate();
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), level = tracing::Level::TRACE)
    )]
    pub(crate) fn set_autosleep_grace(&self, grace: Duration) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.autosleep_grace = grace;
            tracing::info!(
                grace_secs = grace.as_secs_f32(),
                "soft-suspend release grace updated"
            );
        }
        self.wake.set_autosleep_grace(grace);
    }

    fn sync_soft_indicate(&self) {
        let want_on = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.mode.is_armed() && state.indicate_autosleep_led
        };
        let mut guard_slot = self.soft_indicate.lock().unwrap_or_else(|e| e.into_inner());
        if want_on {
            if guard_slot.is_none() {
                *guard_slot = Some(self.status_led.install(
                    SOFT_INDICATE_NAME,
                    LedPriority::SoftIndicate,
                    LedPattern::SolidOn,
                ));
            }
        } else {
            *guard_slot = None;
        }
    }
}
