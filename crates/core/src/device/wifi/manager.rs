//! WiFi Manager trait definition.

use crate::device::wifi::error::WifiError;
use crate::device::wifi::network_info::NetworkInfo;

/// Trait for WiFi management.
///
/// This trait abstracts over platform-specific implementations that enable
/// and disable WiFi connectivity.
///
/// # Lifecycle
///
/// 1. Call [`enable`](WifiManager::enable) when the user wants to connect
///    to a WiFi network.
/// 2. Call [`disable`](WifiManager::disable) when the user disconnects.
///
/// # Example
///
/// ```ignore
/// use cadmus_core::device::wifi::{WifiManager, WifiError};
///
/// # async fn example(wifi_manager: &dyn WifiManager) -> Result<(), WifiError> {
/// // Enable WiFi
/// wifi_manager.enable().await?;
///
/// // ... device is now connected to WiFi ...
///
/// // Disable WiFi
/// wifi_manager.disable().await?;
/// # Ok(())
/// # }
/// ```
#[async_trait::async_trait]
pub trait WifiManager: Send + Sync {
    /// Enables WiFi connectivity.
    ///
    /// # Errors
    ///
    /// Returns [`WifiError`] if enabling fails.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use cadmus_core::device::wifi::WifiManager;
    ///
    /// # async fn example(wifi_manager: &dyn WifiManager) -> Result<(), cadmus_core::device::wifi::WifiError> {
    /// wifi_manager.enable().await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn enable(&self) -> Result<(), WifiError>;

    /// Disables WiFi connectivity.
    ///
    /// # Errors
    ///
    /// Returns [`WifiError`] if disabling fails.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use cadmus_core::device::wifi::WifiManager;
    ///
    /// # async fn example(wifi_manager: &dyn WifiManager) -> Result<(), cadmus_core::device::wifi::WifiError> {
    /// wifi_manager.disable().await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn disable(&self) -> Result<(), WifiError>;

    /// Returns whether Wi-Fi is currently powered/enabled.
    ///
    /// Platform-specific: on Kobo this means the kernel module is loaded and
    /// the network interface is up.
    fn is_enabled(&self) -> bool;

    /// Returns the active connection snapshot, if associated.
    ///
    /// - [`Ok`]`(None)` — Wi-Fi powered/enabled but not associated / no lease yet.
    /// - [`Ok`]`(Some(_))` — connected; `ip` and `essid` are always present.
    /// - [`Err`] — Wi-Fi is disabled, D-Bus/query failure, or connected but IP
    ///   or ESSID missing (inconsistent state).
    ///
    /// Callers must not invoke this when [`is_enabled`](Self::is_enabled) is
    /// false; implementations return [`WifiError::Disabled`] in that case.
    async fn network_info(&self) -> Result<Option<NetworkInfo>, WifiError>;
}

#[async_trait::async_trait]
impl<T: WifiManager + ?Sized> WifiManager for Box<T> {
    async fn enable(&self) -> Result<(), WifiError> {
        (**self).enable().await
    }
    async fn disable(&self) -> Result<(), WifiError> {
        (**self).disable().await
    }
    fn is_enabled(&self) -> bool {
        (**self).is_enabled()
    }
    async fn network_info(&self) -> Result<Option<NetworkInfo>, WifiError> {
        (**self).network_info().await
    }
}
