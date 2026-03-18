//! Platform detection for Kobo USB operations.

use std::env;

/// Detects the platform type from the PLATFORM environment variable.
///
/// The PLATFORM environment variable is set by the Kobo system (rcS/init scripts)
/// and indicates the hardware platform:
///
/// - `"mt8113t-ntx"` → MTK (MediaTek) platform (newer devices)
/// - `"mx6sll-ntx"`, `"mx6sul-ntx"`, `"mx6sl-ntx"`, `"freescale"`, etc. → Legacy platform (older i.MX devices)
///
/// # Panics
///
/// Panics if the PLATFORM environment variable is not set, since we expect
/// the Kobo system to always set it.
///
/// # Example
///
/// ```ignore
/// use cadmus_core::device::usb::kobo::platform::detect_platform;
///
/// std::env::set_var("PLATFORM", "mt8113t-ntx");
/// let platform = detect_platform();
/// assert_eq!(platform, "mt8113t-ntx");
/// ```
pub fn detect_platform() -> String {
    env::var("PLATFORM")
        .expect("PLATFORM environment variable not set - this should be set by the Kobo system")
}
