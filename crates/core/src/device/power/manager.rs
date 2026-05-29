//! Power Manager trait definition.

use crate::device::power::error::PowerError;

/// Trait for device power state management (suspend and resume).
pub trait PowerManager: Send + Sync {
    /// Suspends the device to RAM.
    ///
    /// This method deactivates the touch screen, flushes dirty pages,
    /// and triggers low-power mode.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if any write or sync operation fails.
    fn suspend(&self) -> Result<(), PowerError>;

    /// Resumes the device from suspend.
    ///
    /// This method reactivates the touch screen and applies any necessary
    /// model-specific wake up commands.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if any write operation fails.
    fn resume(&self) -> Result<(), PowerError>;
}
