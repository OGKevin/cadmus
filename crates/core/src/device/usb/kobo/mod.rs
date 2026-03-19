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
//! use cadmus_core::device::{CURRENT_DEVICE, DeviceMetadata};
//!
//! # fn example() -> Result<(), cadmus_core::device::usb::UsbError> {
//! let usb_manager = CURRENT_DEVICE.usb_manager()?;
//! usb_manager.enable()?;
//! // ... USB sharing active ...
//! usb_manager.disable()?;
//! # Ok(())
//! # }
//! ```

use crate::device::metadata::{detect_platform, DeviceMetadata, Platform};
use crate::device::usb::error::UsbError;
use crate::device::usb::manager::UsbManager;

mod operations;

mod legacy;
mod mtk;

use legacy::LegacyUsbManager;
use mtk::MtkUsbManager;

/// Creates a USB manager appropriate for the current platform.
///
/// Detects the platform from the `PLATFORM` environment variable and returns
/// the appropriate implementation:
///
/// - `mt8113t-ntx` → MTK ConfigFS-based manager
/// - All others → Legacy kernel module-based manager
///
/// # Errors
///
/// Returns [`UsbError`] if:
/// - the `PLATFORM` environment variable is not set, or
/// - the MTK UDC cannot be discovered.
///
/// # Example
///
/// ```ignore
/// use cadmus_core::device::{CURRENT_DEVICE, DeviceMetadata};
///
/// # fn example() -> Result<(), cadmus_core::device::usb::UsbError> {
/// let usb_manager = CURRENT_DEVICE.usb_manager()?;
/// # Ok(())
/// # }
/// ```
pub fn create_usb_manager(metadata: DeviceMetadata) -> Result<Box<dyn UsbManager>, UsbError> {
    let platform = detect_platform().map_err(|e| UsbError::DeviceInfo(e.to_string()))?;

    match platform {
        Platform::MT8113TNTX => Ok(Box::new(MtkUsbManager::new(metadata)?)),
        _ => Ok(Box::new(LegacyUsbManager::new(metadata, platform))),
    }
}
