use super::probe::DeviceFeatures;
use crate::input::profile::DeviceProfile;

/// Linux input bus type (`BUS_I2C` from `<linux/input.h>`). Built-in Kobo
/// hardware reports this or [`BUS_HOST`]; [`classify_device`] falls back on
/// both.
const BUS_I2C: u16 = 0x18;

/// Linux input bus type (`BUS_HOST` from `<linux/input.h>`), reported by
/// built-in Kobo hardware such as `gpio-keys`. [`classify_device`] falls
/// back on it, alongside [`BUS_I2C`]; the Kobo Remote is identified by name
/// rather than bustype.
pub(super) const BUS_HOST: u16 = 0x19;

/// The kernel name prefix reported by the Kobo Remote Bluetooth keyboard via
/// `EVIOCGNAME`. The full name varies by host Bluetooth stack — `"Kobo Remote"`
/// on Kobo devices, `"Kobo Remote Keyboard"` on BlueZ/uhid — so a prefix match
/// is used. Arrow keys on this device map to page-turn buttons via the remote
/// [`DeviceProfile`].
const KOBO_REMOTE_NAME_PREFIX: &str = "Kobo Remote";

/// Returns `true` if `name` identifies a Kobo Remote Bluetooth keyboard. The
/// full name varies by host Bluetooth stack, so a prefix match is used.
fn is_kobo_remote_name(name: &str) -> bool {
    name.starts_with(KOBO_REMOTE_NAME_PREFIX)
}

/// Hard-coded kernel names (`EVIOCGNAME`) of built-in Kobo input devices.
///
/// Bus-type and `EVIOCGID` probing alone cannot distinguish some built-in
/// devices: on current Kobo hardware, `gpio-keys` (the page-turn / home /
/// light buttons) and `bd71828-pwrkey` (the PMIC power key) report the
/// *same* `EVIOCGID` tuple (`bus 0x19, vendor 0x1, product 0x1, version
/// 0x100`). The feature scan classifies each device by name so each one is
/// unambiguously identified and the deduplicator does not collapse two
/// distinct devices into one.
///
/// Unknown devices still fall through to the bus-type heuristics in
/// [`classify_device`], so other Kobo models with different touch digitizer
/// names (`ft5x06_ts`, `Wacom Digitizer`, …) continue to work.
const BUILT_IN_DEVICE_NAMES: &[&str] = &["Elan Touchscreen", "bd71828-pwrkey", "gpio-keys"];

/// Returns `true` if `name` is one of the hard-coded built-in device names.
fn is_built_in_name(name: &str) -> bool {
    BUILT_IN_DEVICE_NAMES.contains(&name)
}
/// Classifies a device into a [`DeviceProfile`] from its probed features.
///
/// Dispatch is by hard-coded device name first, with bus-type / event-bit
/// heuristics as a fallback for built-in hardware not listed in
/// [`BUILT_IN_DEVICE_NAMES`] (e.g. other Kobo touch digitizers). Rules, in
/// priority order:
///
/// - Kobo Remote Bluetooth keyboard (name prefix `"Kobo Remote"`) → remote
///   profile: up/down arrows map to raw page-turn.
/// - Hard-coded built-in device name (see [`BUILT_IN_DEVICE_NAMES`]) →
///   built-in profile: power, home, light, scheme-aware page-turn, sleep
///   cover, and pen buttons.
/// - A digitizer (position axes), or a device reporting `BUS_HOST` /
///   `BUS_I2C` → built-in profile. Catches touch digitizers and other
///   built-in buttons on Kobo models whose names are not listed above.
///   Position axes are required for the `EV_ABS` signal because keyboards
///   created through uhid can declare stray vendor axes (e.g. `ABS_MISC`)
///   without being built-in hardware.
/// - Anything else → generic keyboard profile: forward as
///   [`ButtonCode::Keyboard`].
#[cfg_attr(feature = "tracing", tracing::instrument(skip(features)))]
pub(super) fn classify_device(features: &DeviceFeatures) -> DeviceProfile {
    let name = features.name.as_deref().unwrap_or("");
    if is_kobo_remote_name(name) {
        return DeviceProfile::remote();
    }
    if is_built_in_name(name) {
        return DeviceProfile::built_in();
    }
    if features.has_position_axes || features.bustype == BUS_HOST || features.bustype == BUS_I2C {
        return DeviceProfile::built_in();
    }
    DeviceProfile::keyboard()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bluetooth bus type, used only in tests to verify the Kobo Remote is
    /// classified by name: the production code never consults bustype for it.
    const BUS_BLUETOOTH: u16 = 0x05;

    /// Builds a [`DeviceFeatures`] fixture. In the probe, `emits_abs`
    /// without position axes is possible (stray vendor axis), but every
    /// existing test feature set models either a digitizer (abs + position
    /// axes) or a pure key device, so `has_position_axes` mirrors
    /// `emits_abs`.
    fn features(
        bustype: u16,
        name: Option<&str>,
        emits_key: bool,
        emits_abs: bool,
    ) -> DeviceFeatures {
        DeviceFeatures {
            name: name.map(String::from),
            bustype,
            vendor: 0,
            product: 0,
            version: 0,
            emits_key,
            emits_abs,
            has_position_axes: emits_abs,
        }
    }
    #[test]
    fn classify_device_identifies_kobo_remote() {
        let f = features(BUS_BLUETOOTH, Some("Kobo Remote"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::remote());
        let f = features(BUS_BLUETOOTH, Some("Kobo Remote Keyboard"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::remote());
    }
    #[test]
    fn classify_device_identifies_built_in_hardware() {
        let f = features(BUS_HOST, Some("gpio-keys"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::built_in());
        let f = features(BUS_I2C, Some("ft5x06_ts"), true, true);
        assert_eq!(classify_device(&f), DeviceProfile::built_in());
        let f = features(BUS_HOST, Some("bd71828-pwrkey"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::built_in());
        let f = features(BUS_I2C, Some("Elan Touchscreen"), true, true);
        assert_eq!(classify_device(&f), DeviceProfile::built_in());
    }
    /// The Kobo Remote is identified by name; bustype is not required. Real
    /// devices report `BUS_BLUETOOTH`, but the name match is the
    /// authoritative signal.
    #[test]
    fn classify_device_identifies_kobo_remote_regardless_of_bustype() {
        let f = features(0x03, Some("Kobo Remote"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::remote());
    }
    #[test]
    fn classify_device_defaults_generic_keyboards_to_keyboard_profile() {
        let f = features(0x03, Some("Generic Keyboard"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());
        let f = features(BUS_BLUETOOTH, Some("Logitech K380"), true, false);
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());
        let f = features(0, None, true, false);
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());
    }
    /// Reproduces the bug where a hotplugged Bluetooth keyboard whose HID
    /// descriptor declares a stray `ABS_MISC` axis was classified as built-in
    /// hardware: its keys went out as `ButtonCode::Raw` (ignored by views)
    /// and its 'h' key (evdev 35, `SLEEP_COVER[1]`) opened the sleep cover,
    /// locking the screen.
    ///
    /// Fixtures are probed from real hardware: a Targus Folding Ergonomic
    /// Bluetooth Keyboard (uhid: `EV_KEY` + `EV_ABS` declared, the ABS bitmap
    /// holds only bit 40 / `ABS_MISC` — no position axes; Samsung
    /// vendor/product from `EVIOCGID`), and a Framework Laptop 16
    /// consumer-control node, which can also declare stray ABS axes
    /// (volume/scan codes) without being a digitizer.
    #[test]
    fn classify_device_keeps_keyboard_with_stray_abs_axis_generic() {
        let f = DeviceFeatures {
            name: Some("Targus Folding Ergonomic Bluetooth Keyboard".to_string()),
            bustype: BUS_BLUETOOTH,
            vendor: 0x04e8,
            product: 0x7021,
            version: 0x011b,
            emits_key: true,
            emits_abs: true,
            has_position_axes: false,
        };
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());

        let f = DeviceFeatures {
            name: Some("Framework Laptop 16 Keyboard Module - ANSI System Control".to_string()),
            bustype: 0x03,
            vendor: 0x32ac,
            product: 0x0012,
            version: 0x0111,
            emits_key: true,
            emits_abs: true,
            has_position_axes: false,
        };
        assert_eq!(classify_device(&f), DeviceProfile::keyboard());
    }
    #[test]
    fn is_built_in_name_matches_known_devices() {
        assert!(is_built_in_name("Elan Touchscreen"));
        assert!(is_built_in_name("bd71828-pwrkey"));
        assert!(is_built_in_name("gpio-keys"));
    }
    #[test]
    fn is_built_in_name_rejects_unknown_devices() {
        assert!(!is_built_in_name("ft5x06_ts"));
        assert!(!is_built_in_name("Wacom Digitizer"));
        assert!(!is_built_in_name("kx122-accel"));
        assert!(!is_built_in_name(""));
    }
    #[test]
    fn is_kobo_remote_name_matches_kobo_and_bluez_variants() {
        assert!(is_kobo_remote_name("Kobo Remote"));
        assert!(is_kobo_remote_name("Kobo Remote Keyboard"));
    }
    #[test]
    fn is_kobo_remote_name_rejects_unrelated_devices() {
        assert!(!is_kobo_remote_name("Azulle | Lynk Remote"));
        assert!(!is_kobo_remote_name("Generic Keyboard"));
        assert!(!is_kobo_remote_name(""));
    }
}
