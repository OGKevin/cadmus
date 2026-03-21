//! MTK (MediaTek) USB gadget implementation using ConfigFS.
//!
//! This module provides USB mass storage support for MTK-based Kobo devices
//! (platform `mt8113t-ntx`). It uses ConfigFS to configure the USB gadget
//! instead of shell commands.
//!
//! # ConfigFS write convention
//!
//! All values are written with a trailing `\n` to match the behaviour of the
//! `echo` shell command used in the [Linux kernel ConfigFS gadget documentation](https://www.kernel.org/doc/html/latest/usb/gadget_configfs.html)
//! examples. The kernel docs do not explicitly require a newline, but omitting
//! it caused the UDC bind to silently fail on the Libra Colour (MTK platform)
//! in practice.

use crate::device::metadata::DeviceMetadata;
use crate::device::usb::error::UsbError;
use crate::device::usb::kobo::operations::KoboUsbOperations;
use crate::device::usb::manager::UsbManager;
use std::fs;
use std::path::Path;
use tracing::{debug, error, info, warn};

/// Base path for ConfigFS USB gadget configuration.
const CONFIGFS_GADGET_DIR: &str = "/sys/kernel/config/usb_gadget/g1";

/// Path to the UDC directory for auto-discovery.
const UDC_DIR: &str = "/sys/class/udc";

/// Discovers the USB Device Controller (UDC) name.
///
/// Reads the `/sys/class/udc/` directory to find available UDCs. For MTK
/// platforms, this is typically `"11211000.usb"`.
///
/// # Errors
///
/// Returns [`UsbError::Udc`] if no UDC is available or the directory cannot
/// be read.
fn discover_udc() -> Result<String, UsbError> {
    let udc_path = Path::new(UDC_DIR);

    if !udc_path.exists() {
        warn!(path = UDC_DIR, "UDC directory does not exist");
        return Err(UsbError::Udc("UDC directory not found".to_string()));
    }

    let mut entries = fs::read_dir(udc_path).map_err(|e| {
        error!(path = UDC_DIR, error = %e, "Failed to read UDC directory");
        UsbError::Udc(format!("Cannot read UDC directory: {}", e))
    })?;

    let entry = entries
        .next()
        .ok_or_else(|| UsbError::Udc("No UDC available".to_string()))?;
    let entry = entry.map_err(|e| UsbError::Udc(format!("Cannot read UDC entry: {}", e)))?;
    let name = entry
        .file_name()
        .into_string()
        .map_err(|_| UsbError::Udc("UDC name contains invalid UTF-8".to_string()))?;

    debug!(udc_name = %name, "Found UDC");
    Ok(name)
}

/// USB mass storage manager for MTK platforms.
///
/// This implementation configures the USB gadget via ConfigFS, which is the
/// modern approach for MTK-based Kobo devices. It creates the gadget
/// configuration, sets up the mass storage function, and binds to the UDC.
pub struct MtkUsbManager {
    metadata: DeviceMetadata,
    udc: String,
}

impl MtkUsbManager {
    /// Creates a new MTK USB manager.
    ///
    /// Discovers the UDC and prepares for gadget setup. No USB operations
    /// are performed until [`enable`](UsbManager::enable) is called.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError::Udc`] if no UDC is available or the UDC
    /// directory cannot be read.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(metadata)))]
    pub fn new(metadata: DeviceMetadata) -> Result<Self, UsbError> {
        let udc = discover_udc()?;
        info!(
            vendor_id = metadata.vendor_id,
            product_id = metadata.product_id,
            serial_number = %metadata.serial_number,
            partition = %metadata.partition,
            udc = %udc,
            "MtkUsbManager constructed"
        );
        Ok(Self { metadata, udc })
    }

    /// Creates the ConfigFS gadget directory structure.
    fn create_gadget_dirs(&self) -> Result<(), UsbError> {
        debug!("Creating ConfigFS gadget directories");

        let dirs = [
            format!("{}/strings/0x409", CONFIGFS_GADGET_DIR),
            format!("{}/configs/c.1/strings/0x409", CONFIGFS_GADGET_DIR),
            format!("{}/functions/mass_storage.0/lun.0", CONFIGFS_GADGET_DIR),
        ];

        for dir in &dirs {
            fs::create_dir_all(dir).map_err(|e| {
                error!(directory = %dir, error = %e, "Failed to create ConfigFS directory");
                UsbError::GadgetConfig(format!("Cannot create directory {}: {}", dir, e))
            })?;
        }

        Ok(())
    }

    /// Writes gadget configuration to ConfigFS.
    ///
    /// All values are written with a trailing `\n` to match `echo` behaviour from
    /// the kernel documentation examples.
    fn write_gadget_config(&self) -> Result<(), UsbError> {
        debug!("Writing gadget configuration to ConfigFS");

        let base = Path::new(CONFIGFS_GADGET_DIR);

        fs::write(
            base.join("idVendor"),
            format!("0x{:04X}\n", self.metadata.vendor_id),
        )
        .map_err(|e| UsbError::GadgetConfig(format!("Cannot write idVendor: {}", e)))?;

        fs::write(
            base.join("idProduct"),
            format!("0x{:04X}\n", self.metadata.product_id),
        )
        .map_err(|e| UsbError::GadgetConfig(format!("Cannot write idProduct: {}", e)))?;

        let strings = base.join("strings/0x409");
        fs::write(
            strings.join("serialnumber"),
            format!("{}\n", self.metadata.serial_number),
        )
        .map_err(|e| UsbError::GadgetConfig(format!("Cannot write serialnumber: {}", e)))?;

        fs::write(
            strings.join("manufacturer"),
            format!("{}\n", self.metadata.manufacturer),
        )
        .map_err(|e| UsbError::GadgetConfig(format!("Cannot write manufacturer: {}", e)))?;

        fs::write(
            strings.join("product"),
            format!("{}\n", self.metadata.product),
        )
        .map_err(|e| UsbError::GadgetConfig(format!("Cannot write product: {}", e)))?;

        let config_strings = base.join("configs/c.1/strings/0x409");
        fs::write(config_strings.join("configuration"), "KOBOeReader\n")
            .map_err(|e| UsbError::GadgetConfig(format!("Cannot write configuration: {}", e)))?;

        let lun = base.join("functions/mass_storage.0/lun.0");
        fs::write(lun.join("file"), format!("{}\n", self.metadata.partition))
            .map_err(|e| UsbError::GadgetConfig(format!("Cannot write LUN file: {}", e)))?;

        info!(
            vendor_id = self.metadata.vendor_id,
            product_id = self.metadata.product_id,
            partition = %self.metadata.partition,
            "Gadget configuration written"
        );

        Ok(())
    }

    /// Creates symlinks to activate the mass storage function.
    fn activate_function(&self) -> Result<(), UsbError> {
        debug!("Activating mass storage function");

        let src = format!("{}/functions/mass_storage.0", CONFIGFS_GADGET_DIR);
        let dst = format!("{}/configs/c.1/mass_storage.0", CONFIGFS_GADGET_DIR);

        std::os::unix::fs::symlink(&src, &dst).map_err(|e| {
            error!(source = %src, destination = %dst, error = %e, "Failed to create function symlink");
            UsbError::GadgetConfig(format!("Cannot activate function: {}", e))
        })?;

        Ok(())
    }

    /// Binds the gadget to the UDC to enable USB.
    ///
    /// The UDC name is written with a trailing `\n` to match `echo` behaviour from
    /// the kernel documentation examples.
    fn bind_udc(&self) -> Result<(), UsbError> {
        debug!(udc = %self.udc, "Binding to UDC");

        let udc_path = format!("{}/UDC", CONFIGFS_GADGET_DIR);
        fs::write(&udc_path, format!("{}\n", self.udc)).map_err(|e| {
            error!(udc = %self.udc, error = %e, "Failed to bind UDC");
            UsbError::Udc(format!("Cannot bind UDC: {}", e))
        })?;

        info!(udc = %self.udc, "USB gadget enabled");
        Ok(())
    }

    /// Unbinds the gadget from the UDC.
    ///
    /// Writes a bare newline to the UDC attribute, which the kernel interprets
    /// as clearing the binding.
    fn unbind_udc(&self) -> Result<(), UsbError> {
        debug!("Unbinding UDC");

        let udc_path = format!("{}/UDC", CONFIGFS_GADGET_DIR);
        fs::write(&udc_path, "\n").map_err(|e| {
            error!(error = %e, "Failed to unbind UDC");
            UsbError::Udc(format!("Cannot unbind UDC: {}", e))
        })?;

        info!("USB gadget disabled");
        Ok(())
    }

    /// Removes the function symlink.
    fn deactivate_function(&self) -> Result<(), UsbError> {
        debug!("Deactivating mass storage function");

        let link = format!("{}/configs/c.1/mass_storage.0", CONFIGFS_GADGET_DIR);
        if Path::new(&link).exists() {
            fs::remove_file(&link).map_err(|e| {
                error!(path = %link, error = %e, "Failed to remove function symlink");
                UsbError::GadgetConfig(format!("Cannot deactivate function: {}", e))
            })?;
        }

        Ok(())
    }

    /// Removes the gadget directory structure.
    fn remove_gadget_dirs(&self) -> Result<(), UsbError> {
        debug!("Removing ConfigFS gadget directories");

        let dirs = [
            format!("{}/configs/c.1/strings/0x409", CONFIGFS_GADGET_DIR),
            format!("{}/configs/c.1", CONFIGFS_GADGET_DIR),
            format!("{}/functions/mass_storage.0/lun.0", CONFIGFS_GADGET_DIR),
            format!("{}/functions/mass_storage.0", CONFIGFS_GADGET_DIR),
            format!("{}/strings/0x409", CONFIGFS_GADGET_DIR),
            CONFIGFS_GADGET_DIR.to_string(),
        ];

        for dir in &dirs {
            if Path::new(dir).exists() {
                if let Err(e) = fs::remove_dir(dir) {
                    debug!(directory = %dir, error = %e, "Failed to remove directory (may be non-empty)");
                }
            }
        }

        Ok(())
    }
}

impl KoboUsbOperations for MtkUsbManager {
    fn metadata(&self) -> &DeviceMetadata {
        &self.metadata
    }
}

impl UsbManager for MtkUsbManager {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn enable(&self) -> Result<(), UsbError> {
        info!("Enabling MTK USB mass storage");

        self.prepare_for_usb_share()?;
        self.create_gadget_dirs()?;
        self.write_gadget_config()?;
        self.activate_function()?;
        self.bind_udc()?;

        info!("MTK USB mass storage enabled successfully");
        Ok(())
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), ret(level=tracing::Level::TRACE)))]
    fn disable(&self) -> Result<(), UsbError> {
        info!("Disabling MTK USB mass storage");

        self.unbind_udc()?;
        self.deactivate_function()?;
        self.remove_gadget_dirs()?;
        self.check_filesystem()?;
        self.remount_partitions()?;

        info!("MTK USB mass storage disabled successfully");
        Ok(())
    }
}
