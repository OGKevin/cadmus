use crate::device::wifi::{WifiError, WifiManager};

pub struct EmulatorWifiManager;

impl WifiManager for EmulatorWifiManager {
    fn enable(&self) -> Result<(), WifiError> {
        unimplemented!("Emulator doesn't support WiFi");
    }

    fn disable(&self) -> Result<(), WifiError> {
        unimplemented!("Emulator doesn't support WiFi");
    }
}
