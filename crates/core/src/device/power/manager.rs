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

    /// Arms vendor deep-idle peripheral state without writing `/sys/power/state`.
    ///
    /// On Kobo this writes `1` to `/sys/power/state-extended` (touch prep). It does
    /// **not** suspend by itself — callers pair it with autosleep `mem` and releasing
    /// wake locks. Default is a no-op.
    fn arm_deep_idle(&self) -> Result<(), PowerError> {
        Ok(())
    }

    /// Clears vendor deep-idle peripheral state after wake or cancel.
    ///
    /// On Kobo this matches the touch-restore half of [`Self::resume`]. Default is a
    /// no-op.
    fn disarm_deep_idle(&self) -> Result<(), PowerError> {
        Ok(())
    }

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
    fn arm_deep_idle(&self) -> Result<(), PowerError> {
        (**self).arm_deep_idle()
    }
    fn disarm_deep_idle(&self) -> Result<(), PowerError> {
        (**self).disarm_deep_idle()
    }
    fn init_cores(&self) -> Result<(), PowerError> {
        (**self).init_cores()
    }
    fn restore_cores(&self) -> Result<(), PowerError> {
        (**self).restore_cores()
    }
}
