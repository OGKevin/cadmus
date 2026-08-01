//! WiFi error types.

use thiserror::Error;

/// Errors that can occur during WiFi operations.
#[derive(Error, Debug)]
pub enum WifiError {
    /// Failed to read device information.
    #[error("Failed to read device info: {0}")]
    DeviceInfo(String),

    /// Kernel module operation failed.
    #[error("Kernel module operation failed: {0}")]
    KernelModule(String),

    /// WiFi interface operation failed.
    #[error("WiFi interface operation failed: {0}")]
    Interface(String),

    /// ioctl operation failed.
    #[error("ioctl operation failed: {0}")]
    Ioctl(String),

    /// Configuration file error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to acquire lock for WiFi operation.
    #[error("Failed to acquire WiFi lock: {0}")]
    Lock(String),

    /// Wi-Fi is powered down / disabled.
    #[error("Wi-Fi is disabled")]
    Disabled,

    /// D-Bus transport or deserialize failure.
    #[error("D-Bus error: {0}")]
    Dbus(String),

    /// Associated network is missing IP or ESSID.
    #[error("Incomplete network state: {0}")]
    Incomplete(String),
}

/// Clones a [`WifiError`] for test doubles that store `Result`s by value.
///
/// `Io` is recreated from the display string because [`std::io::Error`] is not
/// `Clone`.
#[cfg(any(
    test,
    docsrs,
    all(
        feature = "deviceless",
        not(any(feature = "kobo", feature = "emulator"))
    )
))]
pub(crate) fn clone_wifi_error(error: &WifiError) -> WifiError {
    match error {
        WifiError::DeviceInfo(s) => WifiError::DeviceInfo(s.clone()),
        WifiError::KernelModule(s) => WifiError::KernelModule(s.clone()),
        WifiError::Interface(s) => WifiError::Interface(s.clone()),
        WifiError::Ioctl(s) => WifiError::Ioctl(s.clone()),
        WifiError::Config(s) => WifiError::Config(s.clone()),
        WifiError::Io(e) => WifiError::Io(std::io::Error::other(e.to_string())),
        WifiError::Lock(s) => WifiError::Lock(s.clone()),
        WifiError::Disabled => WifiError::Disabled,
        WifiError::Dbus(s) => WifiError::Dbus(s.clone()),
        WifiError::Incomplete(s) => WifiError::Incomplete(s.clone()),
    }
}
