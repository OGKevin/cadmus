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
}
