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

use crate::device::metadata::DeviceMetadata;
use crate::device::usb::manager::UsbManager;

mod operations;
mod platform;

mod legacy;
mod mtk;

use legacy::LegacyUsbManager;
use mtk::MtkUsbManager;
use platform::detect_platform;

/// Creates a USB manager appropriate for the current platform.
///
/// Detects the platform from the `PLATFORM` environment variable and returns
/// the appropriate implementation:
///
/// - `mt8113t-ntx` → MTK ConfigFS-based manager
/// - All others → Legacy kernel module-based manager
///
/// # Panics
///
/// Panics if the PLATFORM environment variable is not set (see [`detect_platform()`]).
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
pub fn create_usb_manager(metadata: DeviceMetadata) -> Box<dyn UsbManager> {
    let platform = detect_platform();

    if platform == "mt8113t-ntx" {
        Box::new(MtkUsbManager::new(metadata))
    } else {
        Box::new(LegacyUsbManager::new(metadata))
    }
}
