//! WiFi Manager trait definition.

use crate::device::wifi::error::WifiError;

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
/// # fn example(wifi_manager: &dyn WifiManager) -> Result<(), WifiError> {
/// // Enable WiFi
/// wifi_manager.enable()?;
///
/// // ... device is now connected to WiFi ...
///
/// // Disable WiFi
/// wifi_manager.disable()?;
/// # Ok(())
/// # }
/// ```
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
    /// # fn example(wifi_manager: &dyn WifiManager) -> Result<(), cadmus_core::device::wifi::WifiError> {
    /// wifi_manager.enable()?;
    /// # Ok(())
    /// # }
    /// ```
    fn enable(&self) -> Result<(), WifiError>;

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
    /// # fn example(wifi_manager: &dyn WifiManager) -> Result<(), cadmus_core::device::wifi::WifiError> {
    /// wifi_manager.disable()?;
    /// # Ok(())
    /// # }
    /// ```
    fn disable(&self) -> Result<(), WifiError>;
}
