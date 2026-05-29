//! Stub Power Manager implementation for non-Kobo builds.

use crate::device::power::error::PowerError;
use crate::device::power::manager::PowerManager;

/// Stub power manager that panics, you must implement
/// a proper power manager for the device you're working on.
pub struct StubPowerManager;

impl PowerManager for StubPowerManager {
    fn suspend(&self) -> Result<(), PowerError> {
        unimplemented!("There is no implementation for suspending on this build.")
    }

    fn resume(&self) -> Result<(), PowerError> {
        unimplemented!("There is no implementation for resuming on this build.")
    }
}

/// Creates a stub PowerManager instance.
pub fn create_power_manager(
    _model: crate::device::Model,
) -> Result<Box<dyn PowerManager>, PowerError> {
    Ok(Box::new(StubPowerManager))
}
