use crate::device::wifi::{NetworkInfo, WifiError, WifiManager};

pub struct EmulatorWifiManager;

impl WifiManager for EmulatorWifiManager {
    fn enable(&self) -> Result<(), WifiError> {
        unimplemented!("Emulator doesn't support WiFi");
    }

    fn disable(&self) -> Result<(), WifiError> {
        unimplemented!("Emulator doesn't support WiFi");
    }

    fn is_enabled(&self) -> bool {
        false
    }

    fn network_info(&self) -> Result<Option<NetworkInfo>, WifiError> {
        if !self.is_enabled() {
            return Err(WifiError::Disabled);
        }
        Ok(None)
    }
}
