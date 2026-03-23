//! WiFi management for Kobo devices.
//!
//! This module provides native WiFi lifecycle management, replacing the previous
//! shell script-based implementation at `scripts/wifi-enable.sh` and
//! `scripts/wifi-disable.sh`.
//!
//! # Architecture
//!
//! The implementation follows the same patterns as KOReader, with some
//! key differences documented below.
//!
//! ## Power Toggle Strategies
//!
//! Different Kobo devices use different mechanisms to power the WiFi chip:
//!
//! - **`Module`**: Insmod `sdio_wifi_pwr.ko` - most older devices
//! - **`NtxIo`**: ioctl on `/dev/ntx_io` with command 208 - devices with `moal` module
//! - **`Wmt`**: WMT character device - devices with `wlan_drv_gen4m` module
//!
//! ## Module Path Resolution
//!
//! The module path depends on the WiFi module type:
//! - Default: `/drivers/{PLATFORM}/wifi/`
//! - `wlan_drv_gen4m`: `/drivers/{PLATFORM}/mt66xx/`
//!
//! ## Why No DHCP Client?
//!
//! This implementation does NOT run a DHCP client.
//!
//! The rationale is:
//!
//! 1. **Nickel's `dhcpcd` persists**: When Cadmus starts, it does NOT kill Nickel's
//!    `dhcpcd -d -z wlan0` daemon. This daemon continuously manages the DHCP lease
//!    and writes lease files to `/var/db/` on eMMC, which persists across reboots.
//! 2. **Lease stability**: When reconnecting, `dhcpcd` reads the matching lease file
//!    and requests the same IP (DHCP Option 50), avoiding the DHCP server handing
//!    out a different address.
//! 3. **Avoid `udhcpc` pitfalls**: The original shell script used `udhcpc -q` which
//!    exits after obtaining a lease with no persistence mechanism, causing a new IP
//!    on every toggle.
//! 4. **wpa_supplicant handles 802.11 connectivity**: Network configuration is
//!    managed by the platform's `dhcpcd`.
//!
//! ## Features Copied from KOReader
//!
//! The following improvements were adopted from KOReader's implementation:
//!
//! 1. **No DHCP client**: KOReader omits udhcpc when possible.
//! 2. **Module loading**: Uses `insmod_asneeded()` helper that checks
//!    `/proc/modules` before loading to avoid duplicate module loads
//! 3. **250ms delay after insmod**: Matches KOReader's timing for module stabilization
//! 4. **File descriptor cleanup**: Not implemented (KOReader closes non-standard fds
//!    before WiFi operations to avoid issues with USBMS)
//!
//! ## Country Code Handling
//!
//! Regulatory domain is read from `/mnt/onboard/.kobo/Kobo/Kobo eReader.conf`
//! under the `WifiRegulatoryDomain` key. The code parameter is passed to the
//! kernel module as:
//! - `8821cs`: `rtw_country_code=XX`
//! - `moal`: `reg_alpha2=XX`
//!
//! ## Moal-Specific Module Parameters
//!
//! The `moal` module (NXP/Marvell 88W8987) additionally requires:
//! - `mod_para=nxp/wifi_mod_para_sd8987.conf`
//! - Loading `mlan.ko` dependency before the main module

mod types;

#[cfg(target_os = "linux")]
use procfs;

use crate::device::wifi::error::WifiError;
use crate::device::wifi::kobo::types::{PowerToggle, WifiModule, WifiModuleConfig};
use crate::device::wifi::manager::WifiManager;
use nix::ioctl_write_int_bad;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use tracing::{debug, error, info, warn};

const DRIVERS_DIR: &str = "/drivers";
const NTX_IO_PATH: &str = "/dev/ntx_io";
const WMT_WIFI_PATH: &str = "/dev/wmtWifi";
const CONFIG_PATH: &str = "/mnt/onboard/.kobo/Kobo/Kobo eReader.conf";
const WPA_SUPPLICANT_CONF: &str = "/etc/wpa_supplicant/wpa_supplicant.conf";
const WPA_SUPPLICANT_SOCKET: &str = "/var/run/wpa_supplicant";
const WIFI_POST_UP_SCRIPT: &str = "scripts/wifi-post-up.sh";
const WIFI_POST_DOWN_SCRIPT: &str = "scripts/wifi-post-down.sh";
const WIFI_PRE_UP_SCRIPT: &str = "scripts/wifi-pre-up.sh";
const WIFI_PRE_DOWN_SCRIPT: &str = "scripts/wifi-pre-down.sh";

const NTX_IO_WIFI_CTRL: u8 = 208;
ioctl_write_int_bad!(set_ntx_io_wifi_ctrl, NTX_IO_WIFI_CTRL as libc::c_int);

#[cfg(target_os = "linux")]
#[cfg_attr(feature = "otel", tracing::instrument(skip_all, ret(level=tracing::Level::TRACE)))]
fn is_module_loaded(module_name: &str) -> bool {
    procfs::modules()
        .map(|modules| modules.iter().any(|(name, _)| name == module_name))
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
#[cfg_attr(feature = "otel", tracing::instrument(skip_all, ret(level=tracing::Level::TRACE)))]
fn is_module_loaded(_module_name: &str) -> bool {
    unreachable!("is_module_loaded is only implemented on Linux")
}

#[cfg(target_os = "linux")]
fn is_interface_up(interface: &str) -> bool {
    let operstate_path = format!("/sys/class/net/{}/operstate", interface);
    if let Ok(state) = fs::read_to_string(&operstate_path) {
        let state = state.trim();
        return state == "up" || state == "unknown";
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn is_interface_up(_interface: &str) -> bool {
    unreachable!("is_interface_up is only implemented on Linux")
}

pub struct KoboWifiManager {
    config: WifiModuleConfig,
    lock: Mutex<()>,
}

impl KoboWifiManager {
    pub fn new(config: WifiModuleConfig) -> Self {
        Self {
            config,
            lock: Mutex::new(()),
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn run_script(&self, script: &str) {
        if Path::new(script).exists() {
            let output = Command::new(script).output();
            if let Ok(output) = output {
                if !output.status.success() {
                    warn!(
                        script,
                        stderr = %String::from_utf8_lossy(&output.stderr),
                        "WiFi script failed"
                    );
                } else {
                    debug!(script, "WiFi script succeeded");
                }
            }
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(path = %path), ret(level=tracing::Level::TRACE)))]
    fn insmod(&self, path: &str) -> Result<(), WifiError> {
        let output = Command::new("insmod").arg(path).output().map_err(|e| {
            error!(error = %e, path, "Failed to execute insmod");
            WifiError::KernelModule(format!("insmod execution failed: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(path, stderr = %stderr, "insmod failed");
            return Err(WifiError::KernelModule(format!(
                "Failed to load module {}: {}",
                path, stderr
            )));
        }

        debug!(path, "Module loaded successfully");
        Ok(())
    }

    /// Loads a kernel module if it is not already loaded.
    ///
    /// This function is idempotent: if the module is already loaded, this returns `Ok(())`
    /// without attempting to reload it.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(path = %path, module_name = %module_name), ret(level=tracing::Level::TRACE)))]
    fn insmod_asneeded(&self, path: &str, module_name: &str) -> Result<(), WifiError> {
        if !is_module_loaded(module_name) {
            match self.insmod(path) {
                Ok(()) => {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                Err(WifiError::KernelModule(ref msg)) if msg.contains("File exists") => {
                    debug!(
                        module_name,
                        "Module already loaded (insmod returned File exists)"
                    );
                }
                Err(e) => return Err(e),
            }
        } else {
            debug!(module_name, "Module already loaded");
        }
        Ok(())
    }

    /// Loads a kernel module with parameters if it is not already loaded.
    ///
    /// This function is idempotent: if the module is already loaded, this returns `Ok(())`
    /// without attempting to reload it.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(path = %path, module_name = %module_name), ret(level=tracing::Level::TRACE)))]
    fn insmod_asneeded_with_params(
        &self,
        path: &str,
        module_name: &str,
        params: &[&str],
    ) -> Result<(), WifiError> {
        if !is_module_loaded(module_name) {
            let output = Command::new("insmod")
                .arg(path)
                .args(params)
                .output()
                .map_err(|e| {
                    error!(error = %e, path, "Failed to execute insmod");
                    WifiError::KernelModule(format!("insmod execution failed: {}", e))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("File exists") {
                    debug!(
                        module_name,
                        "Module already loaded (insmod returned File exists)"
                    );
                } else {
                    error!(path, stderr = %stderr, "insmod failed");
                    return Err(WifiError::KernelModule(format!(
                        "Failed to load module {}: {}",
                        path, stderr
                    )));
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        } else {
            debug!(module_name, "Module already loaded");
        }
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(module_name = %module_name), ret(level=tracing::Level::TRACE)))]
    fn rmmod(&self, module_name: &str) -> Result<(), WifiError> {
        if !is_module_loaded(module_name) {
            debug!(module_name, "Module not loaded, skipping rmmod");
            return Ok(());
        }

        let output = Command::new("rmmod")
            .arg(module_name)
            .output()
            .map_err(|e| {
                error!(error = %e, module_name, "Failed to execute rmmod");
                WifiError::KernelModule(format!("rmmod execution failed: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(module_name, stderr = %stderr, "rmmod failed");
            return Err(WifiError::KernelModule(format!(
                "Failed to unload {}: {}",
                module_name, stderr
            )));
        }

        debug!(module_name, "Module unloaded successfully");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn power_up_ntx_io(&self) -> Result<(), WifiError> {
        use std::os::unix::fs::OpenOptionsExt;

        let fd = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(NTX_IO_PATH)
            .map_err(|e| {
                error!(error = %e, "Failed to open ntx_io");
                WifiError::Ioctl(format!("Failed to open {}: {}", NTX_IO_PATH, e))
            })?;

        self.ioctl_wifi_ctrl(fd.as_raw_fd(), 1)?;

        info!("WiFi powered up via ntx_io");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn power_down_ntx_io(&self) -> Result<(), WifiError> {
        use std::os::unix::fs::OpenOptionsExt;

        let fd = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(NTX_IO_PATH)
            .map_err(|e| {
                error!(error = %e, "Failed to open ntx_io");
                WifiError::Ioctl(format!("Failed to open {}: {}", NTX_IO_PATH, e))
            })?;

        self.ioctl_wifi_ctrl(fd.as_raw_fd(), 0)?;

        info!("WiFi powered down via ntx_io");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(enable = enable), ret(level=tracing::Level::TRACE)))]
    fn ioctl_wifi_ctrl(&self, fd: std::os::fd::RawFd, enable: u8) -> Result<(), WifiError> {
        let ret = unsafe { set_ntx_io_wifi_ctrl(fd, enable as libc::c_int) }.map_err(|e| {
            WifiError::Ioctl(format!(
                "ioctl CM_WIFI_CTRL with arg {} failed: {}",
                enable, e
            ))
        })?;

        if ret < 0 {
            return Err(WifiError::Ioctl(format!(
                "ioctl CM_WIFI_CTRL with arg {} failed",
                enable
            )));
        }

        debug!(enable, "ioctl CM_WIFI_CTRL succeeded");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn power_up_wmt(&self) -> Result<(), WifiError> {
        let module_path = &self.config.module_path;

        self.insmod_asneeded(&format!("{}/wmt_drv.ko", module_path), "wmt_drv")?;
        self.insmod_asneeded(
            &format!("{}/wmt_chrdev_wifi.ko", module_path),
            "wmt_chrdev_wifi",
        )?;
        self.insmod_asneeded(&format!("{}/wmt_cdev_bt.ko", module_path), "wmt_cdev_bt")?;

        let wifi_module_path = format!("{}/{}.ko", module_path, self.config.module);
        if Path::new(&wifi_module_path).exists() {
            self.insmod_asneeded(&wifi_module_path, self.config.module.as_ref())?;
        }

        fs::write("/proc/driver/wmt_dbg", "0xDB9DB9").ok();
        fs::write("/proc/driver/wmt_dbg", "7 9 0").ok();
        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write("/proc/driver/wmt_dbg", "0xDB9DB9").ok();
        fs::write("/proc/driver/wmt_dbg", "7 9 1").ok();

        fs::write(WMT_WIFI_PATH, "1").map_err(|e| {
            error!(error = %e, "Failed to write to wmtWifi");
            WifiError::Ioctl(format!("Failed to write to {}: {}", WMT_WIFI_PATH, e))
        })?;

        info!("WiFi powered up via WMT");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn power_down_wmt(&self) -> Result<(), WifiError> {
        if !Path::new(WMT_WIFI_PATH).exists() {
            debug!("wmtWifi not present, skipping power down");
            return Ok(());
        }
        match fs::write(WMT_WIFI_PATH, "0") {
            Ok(()) => {
                info!("WiFi powered down via WMT");
                Ok(())
            }
            Err(e) => {
                debug!(error = %e, "wmtWifi power down failed, assuming already off");
                Ok(())
            }
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn power_up_module(&self) -> Result<(), WifiError> {
        let module_path = &self.config.module_path;
        self.insmod_asneeded(
            &format!("{}/sdio_wifi_pwr.ko", module_path),
            "sdio_wifi_pwr",
        )?;
        info!("WiFi powered up via module");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn power_down_module(&self) -> Result<(), WifiError> {
        self.rmmod("sdio_wifi_pwr")?;
        info!("WiFi powered down via module");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip_all, ret(level=tracing::Level::TRACE)))]
    fn read_country_code(&self) -> Option<String> {
        let content = fs::read_to_string(CONFIG_PATH).ok()?;
        for line in content.lines() {
            if line.starts_with("WifiRegulatoryDomain=") {
                return Some(line.split('=').nth(1)?.to_string());
            }
        }
        None
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn build_module_params(&self) -> Vec<String> {
        let mut params = Vec::new();

        if let Some(country_code) = self.read_country_code() {
            match &self.config.module {
                WifiModule::Eight821cs => {
                    params.push(format!("rtw_country_code={}", country_code));
                }
                WifiModule::Moal => {
                    params.push(format!("reg_alpha2={}", country_code));
                }
                _ => {}
            }
        }

        if self.config.module == WifiModule::Moal {
            params.push("mod_para=nxp/wifi_mod_para_sd8987.conf".to_string());
        }

        params
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn load_wifi_module(&self) -> Result<(), WifiError> {
        let module_params = self.build_module_params();
        let platform = std::env::var("PLATFORM").unwrap_or_default();

        if self.config.module == WifiModule::Moal {
            let mlan_path = if Path::new(&format!("{}/{}/mlan.ko", DRIVERS_DIR, platform)).exists()
            {
                format!("{}/{}/mlan.ko", DRIVERS_DIR, platform)
            } else {
                format!("{}/mlan.ko", self.config.module_path)
            };

            if Path::new(&mlan_path).exists() && !is_module_loaded("mlan") {
                self.insmod(&mlan_path)?;
            }
        }

        let wifi_module_path = if Path::new(&format!(
            "{}/{}/{}.ko",
            DRIVERS_DIR, platform, self.config.module
        ))
        .exists()
        {
            format!("{}/{}/{}.ko", DRIVERS_DIR, platform, self.config.module)
        } else {
            format!("{}/{}.ko", self.config.module_path, self.config.module)
        };

        if !Path::new(&wifi_module_path).exists() {
            return Err(WifiError::KernelModule(format!(
                "WiFi module not found: {}",
                wifi_module_path
            )));
        }

        let params: Vec<&str> = module_params.iter().map(|s| s.as_str()).collect();
        self.insmod_asneeded_with_params(&wifi_module_path, self.config.module.as_ref(), &params)?;

        debug!(
            module = %self.config.module,
            "WiFi module loaded"
        );
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(interface = %self.config.interface), ret(level=tracing::Level::TRACE)))]
    fn wait_for_interface(&self) -> Result<(), WifiError> {
        let interface_path = format!("/sys/class/net/{}", self.config.interface);
        let max_attempts = 20;

        for attempt in 0..max_attempts {
            if Path::new(&interface_path).exists() {
                debug!(
                    interface = %self.config.interface,
                    attempt,
                    "Network interface appeared"
                );
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        Err(WifiError::Interface(format!(
            "Network interface {} did not appear after {} attempts",
            self.config.interface, max_attempts
        )))
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn start_wpa_supplicant(&self) -> Result<(), WifiError> {
        use std::process::Command;

        if self.is_wpa_supplicant_running() {
            debug!("wpa_supplicant already running");
            return Ok(());
        }

        let output = Command::new("wpa_supplicant")
            .arg("-D")
            .arg(self.config.wpa_supplicant_driver)
            .arg("-s")
            .arg("-i")
            .arg(&self.config.interface)
            .arg("-c")
            .arg(WPA_SUPPLICANT_CONF)
            .arg("-C")
            .arg(WPA_SUPPLICANT_SOCKET)
            .arg("-B")
            .env("LD_LIBRARY_PATH", "")
            .output()
            .map_err(|e| {
                error!(error = %e, "Failed to execute wpa_supplicant");
                WifiError::Interface(format!("Failed to start wpa_supplicant: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(
                stderr = %stderr,
                "wpa_supplicant may have failed, continuing"
            );
        } else {
            debug!("wpa_supplicant started");
        }

        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip_all, ret(level=tracing::Level::TRACE)))]
    fn is_wpa_supplicant_running(&self) -> bool {
        std::process::Command::new("pkill")
            .args(["-0", "wpa_supplicant"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn stop_wpa_supplicant(&self) -> Result<(), WifiError> {
        let output = std::process::Command::new("wpa_cli")
            .arg("-i")
            .arg(&self.config.interface)
            .arg("terminate")
            .output()
            .map_err(|e| {
                error!(error = %e, "Failed to execute wpa_cli");
                WifiError::Interface(format!("Failed to stop wpa_supplicant: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(stderr = %stderr, "wpa_cli terminate may have failed");
        } else {
            debug!("wpa_supplicant stopped");
        }

        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(interface = %self.config.interface), ret(level=tracing::Level::TRACE)))]
    fn ifconfig_up(&self) -> Result<(), WifiError> {
        let output = std::process::Command::new("ifconfig")
            .arg(&self.config.interface)
            .arg("up")
            .output()
            .map_err(|e| {
                error!(error = %e, "Failed to execute ifconfig");
                WifiError::Interface(format!("Failed to bring up interface: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "ifconfig up failed");
            return Err(WifiError::Interface(format!(
                "Failed to bring up interface: {}",
                stderr
            )));
        }

        debug!(interface = %self.config.interface, "Interface brought up");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(interface = %self.config.interface), ret(level=tracing::Level::TRACE)))]
    fn ifconfig_down(&self) -> Result<(), WifiError> {
        let output = std::process::Command::new("ifconfig")
            .arg(&self.config.interface)
            .arg("down")
            .output()
            .map_err(|e| {
                error!(error = %e, "Failed to execute ifconfig");
                WifiError::Interface(format!("Failed to bring down interface: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(stderr = %stderr, "ifconfig down may have failed");
        } else {
            debug!(interface = %self.config.interface, "Interface brought down");
        }

        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(interface = %self.config.interface), ret(level=tracing::Level::TRACE)))]
    fn wlarm_le_up(&self) -> Result<(), WifiError> {
        if self.config.module != WifiModule::Dhd {
            return Ok(());
        }

        if let Err(e) = std::process::Command::new("wlarm_le")
            .arg("-i")
            .arg(&self.config.interface)
            .arg("up")
            .output()
        {
            warn!(error = %e, "Failed to execute wlarm_le up");
            return Ok(());
        }

        debug!(interface = %self.config.interface, "wlarm_le up succeeded");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(interface = %self.config.interface), ret(level=tracing::Level::TRACE)))]
    fn wlarm_le_down(&self) -> Result<(), WifiError> {
        if self.config.module != WifiModule::Dhd {
            return Ok(());
        }

        if let Err(e) = std::process::Command::new("wlarm_le")
            .arg("-i")
            .arg(&self.config.interface)
            .arg("down")
            .output()
        {
            warn!(error = %e, "Failed to execute wlarm_le down");
            return Ok(());
        }

        debug!(interface = %self.config.interface, "wlarm_le down succeeded");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn power_up(&self) -> Result<(), WifiError> {
        match self.config.power_toggle {
            PowerToggle::Wmt => self.power_up_wmt()?,
            PowerToggle::NtxIo => self.power_up_ntx_io()?,
            PowerToggle::Module => self.power_up_module()?,
        }
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn power_down(&self) -> Result<(), WifiError> {
        match self.config.power_toggle {
            PowerToggle::Wmt => self.power_down_wmt()?,
            PowerToggle::NtxIo => self.power_down_ntx_io()?,
            PowerToggle::Module => self.power_down_module()?,
        }
        Ok(())
    }
}

impl WifiManager for KoboWifiManager {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn enable(&self) -> Result<(), WifiError> {
        let _lock = self
            .lock
            .lock()
            .map_err(|e| WifiError::Lock(format!("Failed to acquire lock: {}", e)))?;

        if is_module_loaded(self.config.module.as_ref()) && is_interface_up(&self.config.interface)
        {
            info!("WiFi already enabled, skipping");
            return Ok(());
        }

        info!(
            module = %self.config.module,
            interface = %self.config.interface,
            "Enabling WiFi"
        );

        self.run_script(WIFI_PRE_UP_SCRIPT);

        self.power_up()?;
        self.load_wifi_module()?;
        self.wait_for_interface()?;
        self.ifconfig_up()?;
        self.wlarm_le_up()?;
        self.start_wpa_supplicant()?;

        self.run_script(WIFI_POST_UP_SCRIPT);

        info!("WiFi enabled successfully");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn disable(&self) -> Result<(), WifiError> {
        let _lock = self
            .lock
            .lock()
            .map_err(|e| WifiError::Lock(format!("Failed to acquire lock: {}", e)))?;

        if !is_module_loaded(self.config.module.as_ref()) {
            info!("WiFi already disabled, skipping");
            return Ok(());
        }

        info!(
            module = %self.config.module,
            "Disabling WiFi"
        );

        self.run_script(WIFI_PRE_DOWN_SCRIPT);

        self.stop_wpa_supplicant()?;
        self.wlarm_le_down()?;
        self.ifconfig_down()?;

        std::thread::sleep(std::time::Duration::from_millis(200));

        if self.config.power_toggle != PowerToggle::Wmt {
            self.rmmod(self.config.module.as_ref())?;
            if self.config.module == WifiModule::Moal {
                self.rmmod("mlan")?;
            }
        }

        self.power_down()?;

        self.run_script(WIFI_POST_DOWN_SCRIPT);

        info!("WiFi disabled successfully");
        Ok(())
    }
}

/// Creates a WiFi manager for the current platform.
///
/// Reads `WIFI_MODULE`, `PLATFORM`, and `INTERFACE` environment variables
/// to determine the appropriate configuration.
///
/// # Errors
///
/// Returns [`WifiError`] if required environment variables are not set.
///
/// # Example
///
/// ```ignore
/// use cadmus_core::device::wifi::create_wifi_manager;
///
/// # fn example() -> Result<(), cadmus_core::device::wifi::WifiError> {
/// let wifi_manager = create_wifi_manager()?;
/// # Ok(())
/// # }
/// ```
#[cfg_attr(feature = "otel", tracing::instrument)]
pub fn create_wifi_manager() -> Result<Box<dyn WifiManager>, WifiError> {
    let config = WifiModuleConfig::from_env().ok_or_else(|| {
        WifiError::DeviceInfo("Missing WIFI_MODULE, PLATFORM, or INTERFACE env".to_string())
    })?;

    Ok(Box::new(KoboWifiManager::new(config)))
}
