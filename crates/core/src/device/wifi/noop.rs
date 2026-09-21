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

#[async_trait::async_trait]
impl WifiManager for NoopWifiManager {
    async fn enable(&self) -> Result<(), WifiError> {
        self.enabled.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn disable(&self) -> Result<(), WifiError> {
        self.enabled.store(false, Ordering::Relaxed);
        Ok(())
    }

    async fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    async fn network_info(&self) -> Result<Option<NetworkInfo>, WifiError> {
        if !self.is_enabled().await {
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enable_disable_are_inert_successes() {
        let wifi = NoopWifiManager::default();
        assert!(!wifi.is_enabled().await);
        wifi.enable().await.unwrap();
        assert!(wifi.is_enabled().await);
        wifi.disable().await.unwrap();
        assert!(!wifi.is_enabled().await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn network_info_when_disabled() {
        let wifi = NoopWifiManager::default();
        assert!(matches!(
            wifi.network_info().await,
            Err(WifiError::Disabled)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn network_info_when_enabled_reports_association() {
        let wifi = NoopWifiManager::default();
        wifi.enable().await.unwrap();
        let info = wifi.network_info().await.unwrap().expect("associated");
        assert_eq!(info.ip, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(info.essid.as_str(), "noop");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wifi_session_acquire_succeeds_without_timeout() {
        let wifi = Arc::new(NoopWifiManager::default());
        let session = WifiSession::new(wifi, WifiMode::Auto);
        let _ = session.acquire("ota-download").await.unwrap();
    }
}
