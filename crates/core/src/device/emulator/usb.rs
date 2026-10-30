use crate::device::usb::{UsbError, UsbManager};

pub struct EmulatorUsbManager;

impl UsbManager for EmulatorUsbManager {
    async fn enable(&self) -> Result<(), UsbError> {
        unimplemented!("USB not available in emulator")
    }

    async fn disable(&self) -> Result<(), UsbError> {
        unimplemented!("USB not available in emulator")
    }
}
