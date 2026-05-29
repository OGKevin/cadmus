//! Kobo Power Manager implementation.
//!
//! This module provides low-level control over Kobo hardware power states.
//! It handles touch screen power state transitions, filesystem buffer flushes,
//! and writing to kernel sysfs nodes to trigger suspend to RAM.

use crate::device::power::error::PowerError;
use crate::device::power::manager::PowerManager;
use crate::device::Model;
use std::fs;
use std::thread;
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
/// // Access via the global device singleton
/// if let Ok(power) = CURRENT_DEVICE.power_manager() {
///     power.suspend().ok();
/// }
/// ```
pub struct KoboPowerManager {
    model: Model,
}

impl KoboPowerManager {
    /// Creates a new `KoboPowerManager` for the specified device model.
    pub fn new(model: Model) -> Self {
        KoboPowerManager { model }
    }
}

impl PowerManager for KoboPowerManager {
    /// Suspends the Kobo device.
    ///
    /// This method performs a sequenced hardware shutdown:
    /// 1. Deactivates the touch screen to prevent phantom touches on wake up.
    /// 2. Sleeps for 2 seconds to allow pending sysfs writes to finalize safely.
    /// 3. Synchronizes filesystem buffers (`sync()`).
    /// 4. Writes `"mem"` to `/sys/power/state` to trigger low-power RAM suspension.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError::Io`] if writing to any of the sysfs control nodes fails.
    fn suspend(&self) -> Result<(), PowerError> {
        tracing::info!("Suspending device to RAM");
        tracing::debug!(path = %STATE_EXTENDED_PATH, value = "1", "Deactivating touch screen");

        fs::write(STATE_EXTENDED_PATH, "1").map_err(|e| {
            tracing::error!(error = %e, path = %STATE_EXTENDED_PATH, "Failed to deactivate touch screen");

            PowerError::Io(e)
        })?;

        tracing::debug!("Sleeping to prevent write errors");
        thread::sleep(Duration::from_secs(2));
        tracing::debug!("Synchronizing file system buffers");

        nix::unistd::sync();

        tracing::debug!(path = %STATE_PATH, value = "mem", "Triggering low power state");

        fs::write(STATE_PATH, "mem").map_err(|e| {
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
    fn resume(&self) -> Result<(), PowerError> {
        tracing::info!("Resuming device");
        tracing::debug!(path = %STATE_EXTENDED_PATH, value = "0", "Reactivating touch screen");

        fs::write(STATE_EXTENDED_PATH, "0").map_err(|e| {
            tracing::error!(error = %e, path = %STATE_EXTENDED_PATH, "Failed to reactivate touch screen");

            PowerError::Io(e)
        })?;

        match self.model {
            Model::GloHD | Model::AuraH2O => {
                tracing::debug!(path = %NEOCMD_PATH, value = "a", "Reinitializing touch controller");

                fs::write(NEOCMD_PATH, "a").map_err(|e| {
                    tracing::warn!(error = %e, path = %NEOCMD_PATH, "Failed to write neocmd");

                    PowerError::Io(e)
                })?;
            }
            _ => {}
        }

        Ok(())
    }
}

/// Creates a Kobo PowerManager instance.
///
/// This factory function instantiates a box-wrapped `KoboPowerManager` implementing
/// the [`PowerManager`] trait.
pub fn create_power_manager(model: Model) -> Result<Box<dyn PowerManager>, PowerError> {
    Ok(Box::new(KoboPowerManager::new(model)))
}
