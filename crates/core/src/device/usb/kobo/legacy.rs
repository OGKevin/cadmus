//! Legacy USB gadget implementation using kernel module loading.
//!
//! This module provides USB mass storage support for older Kobo devices that
//! use the traditional kernel module approach (`g_mass_storage` or
//! `g_file_storage`). It handles the platform-specific module loading and
//! unloading sequences.

use crate::device::metadata::{DeviceMetadata, Platform};
use crate::device::usb::error::UsbError;
use crate::device::usb::kobo::operations::KoboUsbOperations;
use crate::device::usb::manager::UsbManager;
use std::fs;
use std::path::Path;
use std::process::Command;
use tracing::{debug, error, info, warn};

/// Base path for platform-specific kernel modules.
const DRIVERS_DIR: &str = "/drivers";

/// SD card block device partition, present only when a card is inserted.
const SD_CARD_PARTITION: &str = "/dev/mmcblk1p1";

/// USB mass storage manager for legacy platforms.
///
/// This implementation loads kernel modules to enable USB mass storage on
/// older Kobo devices. For each platform it first tries `g_mass_storage.ko`;
/// if that is absent it falls back to `g_file_storage.ko` with
/// platform-specific dependencies:
///
/// - **mx6sll-ntx, mx6ull-ntx**: loads configfs, libcomposite, usb_f_mass_storage deps
/// - **mx6sl-ntx**: no extra dependencies (arcotg_udc is built into the kernel)
/// - **other platforms**: loads arcotg_udc before g_file_storage
pub struct LegacyUsbManager {
    metadata: DeviceMetadata,
    platform: Platform,
}

impl LegacyUsbManager {
    /// Creates a new legacy USB manager.
    ///
    /// Accepts the platform detected by the caller. No USB operations
    /// are performed until [`enable`](UsbManager::enable) is called.
    pub fn new(metadata: DeviceMetadata, platform: Platform) -> Self {
        Self { metadata, platform }
    }

    fn drivers_path(&self) -> String {
        format!("{}/{}", DRIVERS_DIR, self.platform)
    }

    fn has_g_mass_storage(&self) -> bool {
        let path = format!("{}/g_mass_storage.ko", self.drivers_path());
        Path::new(&path).exists()
    }

    /// Builds the `file=` parameter value for `insmod`, including the SD card
    /// partition when one is present (matching the original `usb-enable.sh` behavior).
    fn build_file_param(&self) -> String {
        if Path::new(SD_CARD_PARTITION).exists() {
            debug!(
                sd_partition = SD_CARD_PARTITION,
                "SD card detected, including in USB export"
            );
            format!("{},{}", self.metadata.partition, SD_CARD_PARTITION)
        } else {
            self.metadata.partition.clone()
        }
    }

    fn build_mass_storage_params(&self) -> Vec<String> {
        vec![
            format!("idVendor=0x{:04X}", self.metadata.vendor_id),
            format!("idProduct=0x{:04X}", self.metadata.product_id),
            "iManufacturer=Kobo".to_string(),
            format!("iProduct=eReader-{}", self.metadata.firmware_version),
            format!("iSerialNumber={}", self.metadata.serial_number),
            format!("file={}", self.build_file_param()),
            "stall=1".to_string(),
            "removable=1".to_string(),
        ]
    }

    fn build_file_storage_params(&self) -> Vec<String> {
        match self.platform {
            Platform::MX6SLLNTX | Platform::MX6ULLNTX => self.build_mass_storage_params(),
            _ => {
                vec![
                    format!("vendor=0x{:04X}", self.metadata.vendor_id),
                    format!("product=0x{:04X}", self.metadata.product_id),
                    "vendor_id=Kobo".to_string(),
                    format!("product_id=eReader-{}", self.metadata.firmware_version),
                    format!("SN={}", self.metadata.serial_number),
                    format!("file={}", self.build_file_param()),
                    "stall=1".to_string(),
                    "removable=1".to_string(),
                ]
            }
        }
    }

    fn load_g_mass_storage(&self) -> Result<(), UsbError> {
        info!("Loading g_mass_storage module");

        let module_path = format!("{}/g_mass_storage.ko", self.drivers_path());
        let params = self.build_mass_storage_params();

        let mut cmd = Command::new("insmod");
        cmd.arg(&module_path);
        for param in &params {
            cmd.arg(param);
        }

        let output = cmd.output().map_err(|e| {
            error!(error = %e, "Failed to execute insmod");
            UsbError::KernelModule(format!("insmod execution failed: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "insmod failed");
            return Err(UsbError::KernelModule(format!(
                "Failed to load g_mass_storage: {}",
                stderr
            )));
        }

        info!("g_mass_storage module loaded successfully");
        Ok(())
    }

    fn load_g_file_storage(&self) -> Result<(), UsbError> {
        info!("Loading g_file_storage module with dependencies");

        let gadgets_path = format!("{}/{}/usb/gadget", DRIVERS_DIR, self.platform);

        match self.platform {
            Platform::MX6SLLNTX | Platform::MX6ULLNTX => {
                for module in ["configfs.ko", "libcomposite.ko", "usb_f_mass_storage.ko"] {
                    let path = format!("{}/{}", gadgets_path, module);
                    if Path::new(&path).exists() {
                        debug!(module = %module, "Loading dependency module");
                        let _ = Command::new("insmod").arg(&path).output();
                    }
                }
            }
            Platform::MX6SLNTX => {}
            _ => {
                let arcotg_path = format!("{}/arcotg_udc.ko", gadgets_path);
                if Path::new(&arcotg_path).exists() {
                    debug!("Loading arcotg_udc module");
                    let _ = Command::new("insmod").arg(&arcotg_path).output();
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            }
        }

        let module_path = format!("{}/g_file_storage.ko", gadgets_path);
        let params = self.build_file_storage_params();

        let mut cmd = Command::new("insmod");
        cmd.arg(&module_path);
        for param in &params {
            cmd.arg(param);
        }

        let output = cmd.output().map_err(|e| {
            error!(error = %e, "Failed to execute insmod");
            UsbError::KernelModule(format!("insmod execution failed: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "insmod failed");
            return Err(UsbError::KernelModule(format!(
                "Failed to load g_file_storage: {}",
                stderr
            )));
        }

        info!("g_file_storage module loaded successfully");
        Ok(())
    }

    fn load_usb_module(&self) -> Result<(), UsbError> {
        if self.has_g_mass_storage() {
            self.load_g_mass_storage()
        } else {
            self.load_g_file_storage()
        }
    }

    fn get_loaded_module(&self) -> Option<String> {
        let content = fs::read_to_string("/proc/modules").ok()?;

        for line in content.lines() {
            if line.starts_with("g_mass_storage ") {
                return Some("g_mass_storage".to_string());
            }
            if line.starts_with("g_file_storage ") {
                return Some("g_file_storage".to_string());
            }
        }

        None
    }

    fn unload_usb_modules(&self) -> Result<(), UsbError> {
        let module = match self.get_loaded_module() {
            Some(m) => m,
            None => {
                warn!("No USB module found in /proc/modules");
                return Ok(());
            }
        };

        info!(module = %module, "Unloading USB module");

        let output = Command::new("rmmod").arg(&module).output().map_err(|e| {
            error!(error = %e, "Failed to execute rmmod");
            UsbError::KernelModule(format!("rmmod execution failed: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "rmmod failed");
            return Err(UsbError::KernelModule(format!(
                "Failed to unload {}: {}",
                module, stderr
            )));
        }

        if module == "g_file_storage" {
            match self.platform {
                Platform::MX6SLLNTX | Platform::MX6ULLNTX => {
                    for mod_name in ["usb_f_mass_storage", "libcomposite", "configfs"] {
                        let _ = Command::new("rmmod").arg(mod_name).output();
                    }
                }
                Platform::MX6SLNTX => {}
                _ => {
                    let _ = Command::new("rmmod").arg("arcotg_udc").output();
                }
            }
        }

        info!("USB modules unloaded successfully");
        Ok(())
    }
}

impl KoboUsbOperations for LegacyUsbManager {
    fn metadata(&self) -> &DeviceMetadata {
        &self.metadata
    }
}

impl UsbManager for LegacyUsbManager {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn enable(&self) -> Result<(), UsbError> {
        info!(platform = %self.platform, "Enabling legacy USB mass storage");

        self.prepare_for_usb_share()?;
        self.load_usb_module()?;

        std::thread::sleep(std::time::Duration::from_secs(1));

        info!("Legacy USB mass storage enabled successfully");
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn disable(&self) -> Result<(), UsbError> {
        info!(platform = %self.platform, "Disabling legacy USB mass storage");

        self.unload_usb_modules()?;

        std::thread::sleep(std::time::Duration::from_secs(1));

        self.check_filesystem()?;
        self.remount_partitions()?;

        info!("Legacy USB mass storage disabled successfully");
        Ok(())
    }
}
