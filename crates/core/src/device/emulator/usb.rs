use crate::device::usb::{UsbError, UsbManager};

pub struct EmulatorUsbManager;

impl UsbManager for EmulatorUsbManager {
    fn enable(&self) -> Result<(), UsbError> {
        unimplemented!("USB not available in emulator")
    }

    fn disable(&self) -> Result<(), UsbError> {
        unimplemented!("USB not available in emulator")
    }
}
