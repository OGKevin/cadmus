//! WiFi types for Kobo devices.

use std::env;

/// Power toggle mechanism for WiFi chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerToggle {
    /// Use kernel module (sdio_wifi_pwr.ko).
    Module,
    /// Use ntx_io ioctl interface.
    NtxIo,
    /// Use wmt character device.
    Wmt,
}

/// WiFi module configuration.
#[derive(Debug, Clone)]
pub struct WifiModuleConfig {
    /// The WiFi kernel module name (e.g., "moal", "wlan_drv_gen4m", "8821cs", "dhd").
    pub module_name: String,
    /// The power toggle mechanism used by this module.
    pub power_toggle: PowerToggle,
    /// The WPA supplicant driver to use.
    pub wpa_supplicant_driver: &'static str,
    /// The network interface name (e.g., "wlan0", "eth0").
    pub interface: String,
    /// The base path for kernel modules.
    pub module_path: String,
}

impl WifiModuleConfig {
    /// Creates WiFi configuration from environment variables.
    ///
    /// Reads `WIFI_MODULE`, `PLATFORM`, and `INTERFACE` environment variables
    /// to determine the appropriate configuration.
    ///
    /// These environment variables are set by the cadmus.sh startup script by
    /// getting them from Nickel's environment variables.
    pub fn from_env() -> Option<Self> {
        let wifi_module = env::var("WIFI_MODULE").ok()?;
        let platform = env::var("PLATFORM").ok()?;
        let interface = env::var("INTERFACE").ok()?;

        let (power_toggle, wpa_supplicant_driver, module_path) = match wifi_module.as_str() {
            "moal" => (
                PowerToggle::NtxIo,
                "nl80211",
                format!("/drivers/{}/wifi", platform),
            ),
            "wlan_drv_gen4m" => (
                PowerToggle::Wmt,
                "nl80211",
                format!("/drivers/{}/mt66xx", platform),
            ),
            _ => (
                PowerToggle::Module,
                "wext",
                format!("/drivers/{}/wifi", platform),
            ),
        };

        Some(Self {
            module_name: wifi_module,
            power_toggle,
            wpa_supplicant_driver,
            interface,
            module_path,
        })
    }
}
