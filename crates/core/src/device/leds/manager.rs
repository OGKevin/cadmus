//! Device LEDs trait definition.

use crate::device::leds::error::LedsError;

/// Trait for turning the device status LED on or off.
pub trait DeviceLeds: Send + Sync {
    /// Turns the status LED on.
    ///
    /// # Errors
    ///
    /// Returns [`LedsError`] when the LED brightness sysfs node cannot be written.
    fn on(&self) -> Result<(), LedsError>;

    /// Turns the status LED off.
    ///
    /// # Errors
    ///
    /// Returns [`LedsError`] when the LED brightness sysfs node cannot be written.
    fn off(&self) -> Result<(), LedsError>;

    /// Sets the status LED to on (`true`) or off (`false`).
    ///
    /// # Errors
    ///
    /// Returns [`LedsError`] when the LED brightness sysfs node cannot be written.
    fn set_on(&self, on: bool) -> Result<(), LedsError> {
        if on { self.on() } else { self.off() }
    }
}

impl<T: DeviceLeds + ?Sized> DeviceLeds for Box<T> {
    fn on(&self) -> Result<(), LedsError> {
        (**self).on()
    }

    fn off(&self) -> Result<(), LedsError> {
        (**self).off()
    }

    fn set_on(&self, on: bool) -> Result<(), LedsError> {
        (**self).set_on(on)
    }
}
