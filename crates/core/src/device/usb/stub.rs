//! Stub USB implementation for non-Kobo builds.

use crate::device::usb::error::UsbError;
use crate::device::usb::manager::UsbManager;

/// Stub USB manager that panics on all operations.
pub struct StubUsbManager;

impl UsbManager for StubUsbManager {
    fn enable(&self) -> Result<(), UsbError> {
        unimplemented!("USB operations not available in this build")
    }

    fn disable(&self) -> Result<(), UsbError> {
        unimplemented!("USB operations not available in this build")
    }
}
