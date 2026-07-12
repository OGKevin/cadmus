//! USB mass storage gadget management for Kobo devices.
//!
//! This module provides native USB lifecycle management, replacing the previous
//! shell script-based implementation. It supports two backends:
//!
//! - **MTK (MediaTek)**: Uses ConfigFS for newer devices (platform `mt8113t-ntx`).
//! - **Legacy**: Uses kernel module loading via `insmod`/`rmmod` for older devices.
//!
//! # Example
//!
//! ```ignore
//! use cadmus_core::device::metadata::DeviceMetadata;
//!
//! # fn example() -> Result<(), cadmus_core::device::usb::UsbError> {
//! let device = DeviceMetadata::detect();
//! let usb_manager = device.usb_manager()?;
//! usb_manager.enable()?;
//! // ... USB sharing active ...
//! usb_manager.disable()?;
//! # Ok(())
//! # }
//! ```

use crate::device::metadata::{DeviceMetadata, Platform, detect_platform};
use crate::device::usb::{UsbError, UsbManager};

mod operations;

mod legacy;
mod mtk;

use legacy::LegacyUsbManager;
use mtk::MtkUsbManager;

/// Concrete USB manager for Kobo devices.
///
/// Dispatches to the appropriate platform backend at construction time:
/// - [`MtkUsbManager`] for `mt8113t-ntx` (MediaTek) platforms
/// - [`LegacyUsbManager`] for all other platforms
pub enum KoboUsbManager {
    /// MTK ConfigFS-based manager for newer Kobo devices.
    Mtk(MtkUsbManager),
    /// Kernel module-based manager for older Kobo devices.
    Legacy(LegacyUsbManager),
}

impl KoboUsbManager {
    /// Creates a `KoboUsbManager` appropriate for the current platform.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError`] if:
    /// - the `PLATFORM` environment variable is not set, or
    /// - the MTK UDC cannot be discovered.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(metadata)))]
    pub fn new(metadata: DeviceMetadata) -> Result<Self, UsbError> {
        let platform = detect_platform().map_err(|e| UsbError::DeviceInfo(e.to_string()))?;
        match platform {
            Platform::MT8113TNTX => Ok(Self::Mtk(MtkUsbManager::new(metadata)?)),
            _ => Ok(Self::Legacy(LegacyUsbManager::new(metadata, platform))),
        }
    }
}

impl UsbManager for KoboUsbManager {
    fn enable(&self) -> Result<(), UsbError> {
        match self {
            Self::Mtk(m) => m.enable(),
            Self::Legacy(m) => m.enable(),
        }
    }

    fn disable(&self) -> Result<(), UsbError> {
        match self {
            Self::Mtk(m) => m.disable(),
            Self::Legacy(m) => m.disable(),
        }
    }
}
