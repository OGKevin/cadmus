//! WiFi types for Kobo devices.

use std::env;
use std::fmt;
use std::str::FromStr;
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WifiModule {
    Moal,
    WlanDrvGen4m,
    Eight821cs,
    Dhd,
    Other(String),
}

impl fmt::Display for WifiModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WifiModule::Moal => write!(f, "moal"),
            WifiModule::WlanDrvGen4m => write!(f, "wlan_drv_gen4m"),
            WifiModule::Eight821cs => write!(f, "8821cs"),
            WifiModule::Dhd => write!(f, "dhd"),
            WifiModule::Other(name) => write!(f, "{}", name),
        }
    }
}

impl AsRef<str> for WifiModule {
    fn as_ref(&self) -> &str {
        match self {
            WifiModule::Moal => "moal",
            WifiModule::WlanDrvGen4m => "wlan_drv_gen4m",
            WifiModule::Eight821cs => "8821cs",
            WifiModule::Dhd => "dhd",
            WifiModule::Other(name) => name,
        }
    }
}

impl From<&str> for WifiModule {
    fn from(s: &str) -> Self {
        match s {
            "moal" => WifiModule::Moal,
            "wlan_drv_gen4m" => WifiModule::WlanDrvGen4m,
            "8821cs" => WifiModule::Eight821cs,
            "dhd" => WifiModule::Dhd,
            unknown_module => {
                warn!(module = %unknown_module, "Unknown WiFi module");
                WifiModule::Other(unknown_module.to_string())
            }
        }
    }
}

impl FromStr for WifiModule {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(WifiModule::from(s))
    }
}

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
    pub module: WifiModule,
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
    /// getting them from Nickel's environment variables. Except for the PLATFORM variable,
    /// which is set by some early Kobo boot script.
    pub fn from_env() -> Option<Self> {
        let wifi_module = env::var("WIFI_MODULE").ok()?;
        let platform = env::var("PLATFORM").ok()?;
        let interface = env::var("INTERFACE").ok()?;

        let wifi_module = WifiModule::from(wifi_module.as_str());
        let (power_toggle, wpa_supplicant_driver, module_path) = match wifi_module {
            WifiModule::Moal => (
                PowerToggle::NtxIo,
                "nl80211",
                format!("/drivers/{}/wifi", platform),
            ),
            WifiModule::WlanDrvGen4m => (
                PowerToggle::Wmt,
                "nl80211",
                format!("/drivers/{}/mt66xx", platform),
            ),
            WifiModule::Other(_) => (
                PowerToggle::Module,
                "wext",
                format!("/drivers/{}/wifi", platform),
            ),
            _ => (
                PowerToggle::Module,
                "wext",
                format!("/drivers/{}/wifi", platform),
            ),
        };

        Some(Self {
            module: wifi_module,
            power_toggle,
            wpa_supplicant_driver,
            interface,
            module_path,
        })
    }
}
