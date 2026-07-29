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
