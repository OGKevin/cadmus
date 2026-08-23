//! Cooperative no-op WiFi manager for emulator and hosts without a real radio.

use super::{Essid, NetworkInfo, WifiError, WifiManager};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative WiFi manager: enable and disable succeed without kernel or D-Bus work.
///
/// When enabled, reports a stub association so [`super::session::WifiSession::acquire`]
/// completes without waiting for an external NetUp signal. Distinct from the private
/// `UnavailableWifi` fallback used when a device has no WiFi manager at all.
#[derive(Debug, Default)]
pub struct NoopWifiManager {
    enabled: AtomicBool,
}

impl NoopWifiManager {
    fn stub_network_info() -> NetworkInfo {
        NetworkInfo {
            ip: IpAddr::from([127, 0, 0, 1]),
            essid: Essid::new("noop"),
        }
    }
}

impl WifiManager for NoopWifiManager {
    fn enable(&self) -> Result<(), WifiError> {
        self.enabled.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn disable(&self) -> Result<(), WifiError> {
        self.enabled.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn network_info(&self) -> Result<Option<NetworkInfo>, WifiError> {
        if !self.is_enabled() {
            return Err(WifiError::Disabled);
        }
        Ok(Some(Self::stub_network_info()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::wifi::session::WifiSession;
    use crate::settings::WifiMode;
    use std::sync::Arc;

    #[test]
    fn enable_disable_are_inert_successes() {
        let wifi = NoopWifiManager::default();
        assert!(!wifi.is_enabled());
        wifi.enable().unwrap();
        assert!(wifi.is_enabled());
        wifi.disable().unwrap();
        assert!(!wifi.is_enabled());
    }

    #[test]
    fn network_info_when_disabled() {
        let wifi = NoopWifiManager::default();
        assert!(matches!(wifi.network_info(), Err(WifiError::Disabled)));
    }

    #[test]
    fn network_info_when_enabled_reports_association() {
        let wifi = NoopWifiManager::default();
        wifi.enable().unwrap();
        let info = wifi.network_info().unwrap().expect("associated");
        assert_eq!(info.ip, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(info.essid.as_str(), "noop");
    }

    #[test]
    fn wifi_session_acquire_succeeds_without_timeout() {
        let wifi = Arc::new(NoopWifiManager::default());
        let session = WifiSession::new(wifi, WifiMode::Auto);
        let _ = session.acquire("ota-download").unwrap();
    }
}
