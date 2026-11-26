//! Kobo Power Manager implementation.
//!
//! This module provides low-level control over Kobo hardware power states.
//! It handles touch screen power state transitions, filesystem buffer flushes,
//! and writing to kernel sysfs nodes to trigger suspend to RAM.

use crate::device::kobo::Model;
use crate::device::power::{PowerError, PowerManager};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const STATE_EXTENDED_PATH: &str = "/sys/power/state-extended";
const STATE_PATH: &str = "/sys/power/state";
const NEOCMD_PATH: &str = "/sys/devices/virtual/input/input1/neocmd";

/// Kobo-specific power manager.
///
/// Manages the low-level hardware sleep/wake cycle on Kobo e-reader devices.
/// It interacts directly with the Linux sysfs interface to manage screen power
/// and RAM suspension.
///
/// # Example
///
/// ```ignore
/// use cadmus_core::device::Model;
/// use cadmus_core::device::power::PowerManager;
///
/// let model = Model::detect();
/// if let Ok(power) = model.power_manager() {
///     power.suspend().ok();
/// }
/// ```
pub struct KoboPowerManager {
    model: Model,
    initial_cpu_states: Mutex<Vec<PathBuf>>,
}

impl KoboPowerManager {
    /// Creates a new `KoboPowerManager` for the specified device model.
    pub fn new(model: Model) -> Self {
        KoboPowerManager {
            model,
            initial_cpu_states: Mutex::new(Vec::new()),
        }
    }

    async fn write_state_extended(&self, value: &str) -> Result<(), PowerError> {
        tracing::debug!(path = %STATE_EXTENDED_PATH, value, "Writing state-extended");
        tokio::fs::write(STATE_EXTENDED_PATH, value).await.map_err(|e| {
            tracing::error!(error = %e, path = %STATE_EXTENDED_PATH, value, "Failed to write state-extended");
            PowerError::Io(e)
        })
    }

    async fn restore_touch_controller(&self) -> Result<(), PowerError> {
        match self.model {
            Model::GloHD | Model::AuraH2O => {
                tracing::debug!(path = %NEOCMD_PATH, value = "a", "Reinitializing touch controller");
                tokio::fs::write(NEOCMD_PATH, "a").await.map_err(|e| {
                    tracing::warn!(error = %e, path = %NEOCMD_PATH, "Failed to write neocmd");
                    PowerError::Io(e)
                })
            }
            _ => Ok(()),
        }
    }
}

impl PowerManager for KoboPowerManager {
    /// Arms peripherals, waits for pending sysfs writes, then flushes buffers.
    ///
    /// The two-second settle uses an async timer and `sync()` runs on the
    /// blocking pool, so preparation never stalls the runtime.
    async fn prepare_suspend(&self) -> Result<(), PowerError> {
        self.arm_deep_idle().await?;

        tracing::debug!("Sleeping to prevent write errors");
        tokio::time::sleep(Duration::from_secs(2)).await;
        tracing::debug!("Synchronizing file system buffers");

        crate::runtime::spawn_blocking(nix::unistd::sync)
            .await
            .map_err(|error| PowerError::Io(std::io::Error::other(error)))?;

        Ok(())
    }

    /// Triggers low-power RAM suspension by writing `"mem"` to `/sys/power/state`.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError::Io`] if the suspend trigger write fails. A
    /// submitted write cannot be cancelled by dropping this future.
    async fn suspend(&self) -> Result<(), PowerError> {
        tracing::info!("Suspending device to RAM");
        tracing::debug!(path = %STATE_PATH, value = "mem", "Triggering low power state");

        tokio::fs::write(STATE_PATH, "mem").await.map_err(|e| {
            tracing::error!(error = %e, path = %STATE_PATH, "Failed to write suspend trigger");

            PowerError::Io(e)
        })?;

        Ok(())
    }

    /// Resumes the Kobo device.
    ///
    /// This method performs the following wakeup tasks:
    /// 1. Reactivates the touch screen by writing `"0"` to the state-extended node.
    /// 2. If the model is a `GloHD` or `AuraH2O`, writes `"a"` to the `neocmd` node
    ///    to re-initialize the touch controller.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError::Io`] if writing to any of the sysfs wake up nodes fails.
    async fn resume(&self) -> Result<(), PowerError> {
        tracing::info!("Resuming device");
        self.disarm_deep_idle().await
    }

    async fn arm_deep_idle(&self) -> Result<(), PowerError> {
        tracing::debug!("Arming Kobo deep-idle peripheral state");
        self.write_state_extended("1").await
    }

    async fn disarm_deep_idle(&self) -> Result<(), PowerError> {
        tracing::debug!("Clearing Kobo deep-idle peripheral state");
        self.write_state_extended("0").await?;
        self.restore_touch_controller().await?;

        Ok(())
    }

    async fn init_cores(&self) -> Result<(), PowerError> {
        let discovered = crate::device::power::discover_cores(Path::new("/sys/devices/system/cpu"))
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "Failed to discover CPU cores");
                PowerError::Io(e)
            })?;

        let mut modified_cores = Vec::new();
        let mut first_error: Option<PowerError> = None;

        for (online_path, initial_state) in discovered {
            if initial_state == "0" {
                tracing::info!(path = ?online_path, "Enabling offline CPU core");
                match tokio::fs::write(&online_path, "1").await {
                    Ok(()) => modified_cores.push(online_path),
                    Err(e) => {
                        tracing::error!(path = ?online_path, error = %e, "Failed to enable CPU core");
                        if first_error.is_none() {
                            first_error = Some(PowerError::Io(e));
                        }
                    }
                }
            }
        }

        let mut states = self
            .initial_cpu_states
            .lock()
            .map_err(|_| PowerError::LockPoisoned)?;
        *states = modified_cores;

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn restore_cores(&self) -> Result<(), PowerError> {
        let states = self
            .initial_cpu_states
            .lock()
            .map_err(|_| PowerError::LockPoisoned)?
            .clone();

        let mut first_error: Option<PowerError> = None;

        for path in states {
            tracing::info!(path = ?path, "Disabling CPU core on exit");
            if let Err(e) = tokio::fs::write(&path, "0").await {
                tracing::error!(path = ?path, error = %e, "Failed to restore CPU core state");
                if first_error.is_none() {
                    first_error = Some(PowerError::Io(e));
                }
            }
        }

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
