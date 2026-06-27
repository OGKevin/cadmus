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

    /// Initializes and enables all available CPU cores on startup.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if scanning or enabling fails.
    fn init_cores(&self) -> Result<(), PowerError> {
        Ok(())
    }

    /// Restores CPU cores to their initial state on shutdown.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if writing the saved states fails.
    fn restore_cores(&self) -> Result<(), PowerError> {
        Ok(())
    }
}

impl<T: PowerManager + ?Sized> PowerManager for Box<T> {
    fn suspend(&self) -> Result<(), PowerError> {
        (**self).suspend()
    }
    fn resume(&self) -> Result<(), PowerError> {
        (**self).resume()
    }
    fn init_cores(&self) -> Result<(), PowerError> {
        (**self).init_cores()
    }
    fn restore_cores(&self) -> Result<(), PowerError> {
        (**self).restore_cores()
    }
}
