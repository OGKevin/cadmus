//! Stub WiFi implementation for non-Kobo builds.

use crate::device::wifi::error::WifiError;
use crate::device::wifi::manager::WifiManager;

/// Stub WiFi manager that panics on all operations.
///
/// If a device doesn't support WiFi, it's implementation must reflect that.
pub struct StubWifiManager;

impl WifiManager for StubWifiManager {
    fn enable(&self) -> Result<(), WifiError> {
        unimplemented!("There is no implementation for enabling WiFi on this build.")
    }

    fn disable(&self) -> Result<(), WifiError> {
        unimplemented!("There isn o implementation for disabling WiFi on this build.")
    }
}

pub fn create_wifi_manager() -> Result<Box<dyn WifiManager>, WifiError> {
    Ok(Box::new(StubWifiManager))
}
