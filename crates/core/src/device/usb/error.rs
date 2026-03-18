//! USB error types.

use thiserror::Error;

/// Errors that can occur during USB operations.
#[derive(Error, Debug)]
pub enum UsbError {
    /// Failed to read device information.
    #[error("Failed to read device info: {0}")]
    DeviceInfo(String),

    /// USB gadget configuration failed.
    #[error("USB gadget configuration failed: {0}")]
    GadgetConfig(String),

    /// Kernel module operation failed.
    #[error("Kernel module operation failed: {0}")]
    KernelModule(String),

    /// Partition operation failed.
    #[error("Partition operation failed: {0}")]
    Partition(String),

    /// Filesystem check failed.
    #[error("Filesystem check failed: {0}")]
    Filesystem(String),

    /// UDC not available.
    #[error("UDC not available: {0}")]
    Udc(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
