//! Power Manager trait definition.

use crate::device::power::error::PowerError;

/// Trait for device power state management (suspend and resume).
///
/// Methods are futures because implementations perform sysfs I/O and timers.
/// The `Send` bounds match the process runtime's worker pool; implementations
/// must keep synchronous locks out of `.await` scopes. Used generically rather
/// than as `dyn`, so the stable RPITIT form avoids boxing.
pub trait PowerManager: Send + Sync {
    /// Prepares peripherals and flushes buffers before [`Self::suspend`].
    ///
    /// On Kobo this arms the touch peripheral state, waits for pending sysfs
    /// writes to settle, and calls `sync()` on the blocking pool. It does not
    /// trigger RAM suspension. Default is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if arming or the buffer flush fails; partial
    /// preparation must be unwound by the caller.
    fn prepare_suspend(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        async { Ok(()) }
    }

    /// Triggers low-power RAM suspension (`/sys/power/state` = `"mem"`).
    ///
    /// The future resolves only once the kernel write completes (or fails):
    /// dropping it does not stop a submitted write, so the caller owns the
    /// commit through wake and peripheral cleanup.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if the suspend trigger write fails.
    fn suspend(&self) -> impl Future<Output = Result<(), PowerError>> + Send;

    /// Reactivates peripherals after [`Self::suspend`].
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if any wake-up write fails.
    fn resume(&self) -> impl Future<Output = Result<(), PowerError>> + Send;

    /// Arms vendor deep-idle peripheral state without writing `/sys/power/state`.
    ///
    /// On Kobo this writes `1` to `/sys/power/state-extended` (touch prep). It does
    /// **not** suspend by itself — callers pair it with autosleep `mem` and releasing
    /// wake locks. Default is a no-op.
    fn arm_deep_idle(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        async { Ok(()) }
    }

    /// Clears vendor deep-idle peripheral state after wake or cancel.
    ///
    /// On Kobo this matches the touch-restore half of [`Self::resume`]. Default is a
    /// no-op.
    fn disarm_deep_idle(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        async { Ok(()) }
    }

    /// Initializes and enables all available CPU cores on startup.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if scanning or enabling fails.
    fn init_cores(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        async { Ok(()) }
    }

    /// Restores CPU cores to their initial state on shutdown.
    ///
    /// # Errors
    ///
    /// Returns [`PowerError`] if writing the saved states fails.
    fn restore_cores(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        async { Ok(()) }
    }
}

impl<T: PowerManager> PowerManager for Box<T> {
    fn prepare_suspend(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        (**self).prepare_suspend()
    }

    fn suspend(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        (**self).suspend()
    }

    fn resume(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        (**self).resume()
    }

    fn arm_deep_idle(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        (**self).arm_deep_idle()
    }

    fn disarm_deep_idle(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        (**self).disarm_deep_idle()
    }

    fn init_cores(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        (**self).init_cores()
    }

    fn restore_cores(&self) -> impl Future<Output = Result<(), PowerError>> + Send {
        (**self).restore_cores()
    }
}
