//! Common USB operations trait for Kobo devices.

use crate::device::metadata::DeviceMetadata;
use crate::device::usb::error::UsbError;
use std::fs;
use std::process::Command;
use tracing::{debug, error, info, warn};

use nix::unistd::sync;

#[cfg(target_os = "linux")]
use nix::mount::{mount, umount2, MntFlags, MsFlags};
#[cfg(target_os = "linux")]
use procfs;
#[cfg(target_os = "linux")]
use std::path::Path;

/// Trait providing common USB operations for Kobo devices.
///
/// Implementors must provide access to the device metadata via the
/// [`metadata`](KoboUsbOperations::metadata) method. All other methods have
/// default implementations that use the metadata.
///
/// # Example
///
/// ```ignore
/// use cadmus_core::device::usb::kobo::operations::KoboUsbOperations;
/// use cadmus_core::device::DeviceMetadata;
///
/// struct MyUsbManager {
///     metadata: DeviceMetadata,
/// }
///
/// impl KoboUsbOperations for MyUsbManager {
///     fn metadata(&self) -> &DeviceMetadata {
///         &self.metadata
///     }
/// }
///
/// # fn example(manager: &MyUsbManager) -> Result<(), cadmus_core::device::usb::UsbError> {
/// manager.prepare_for_usb_share()?;
/// # Ok(())
/// # }
/// ```
pub trait KoboUsbOperations {
    /// Provides access to the device metadata.
    ///
    /// Implementors must return a reference to their [`DeviceMetadata`], which
    /// is used by the default implementations of other trait methods.
    fn metadata(&self) -> &DeviceMetadata;

    /// Syncs filesystem buffers and drops caches.
    ///
    /// This function ensures all pending writes are flushed to disk before
    /// unmounting partitions for USB mass storage mode.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError::Io`] if the sync command or cache drop fails.
    fn sync_and_drop_caches(&self) -> Result<(), UsbError> {
        debug!("Syncing filesystem buffers");

        sync();

        fs::write("/proc/sys/vm/drop_caches", "3").map_err(|e| {
            error!(error = %e, "Failed to drop caches");
            UsbError::Io(e)
        })?;

        Ok(())
    }

    /// Checks if a mount point is currently mounted.
    ///
    /// Uses procfs to read mount information and checks if the
    /// mount_point appears in the list.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError::Io`] if mount information cannot be read.
    /// Callers should treat a read failure as an error rather than assuming
    /// the filesystem is unmounted.
    #[cfg(target_os = "linux")]
    fn is_mounted(&self, mount_point: &str) -> Result<bool, UsbError> {
        procfs::mounts()
            .map(|mounts| mounts.iter().any(|m| m.fs_file == mount_point))
            .map_err(|e| {
                warn!(error = %e, mount_point = %mount_point, "Failed to read mounts");
                UsbError::Io(std::io::Error::other(format!(
                    "Failed to read mount info: {}",
                    e
                )))
            })
    }

    #[cfg(not(target_os = "linux"))]
    fn is_mounted(&self, _mount_point: &str) -> Result<bool, UsbError> {
        Ok(false)
    }

    /// Unmounts a partition lazily.
    ///
    /// Detaches the filesystem immediately but cleans up references in the background.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError::Partition`] if the unmount operation fails.
    #[cfg(target_os = "linux")]
    fn unmount_partition(&self, mount_point: &str) -> Result<(), UsbError> {
        debug!(mount_point = %mount_point, "Unmounting partition");

        umount2(mount_point, MntFlags::MNT_DETACH).map_err(|e| {
            error!(mount_point = %mount_point, error = %e, "Failed to unmount partition");
            UsbError::Partition(format!("Failed to unmount {}: {}", mount_point, e))
        })?;

        info!(mount_point = %mount_point, "Unmounted partition");
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn unmount_partition(&self, _mount_point: &str) -> Result<(), UsbError> {
        Err(UsbError::Partition(
            "Unmount not supported on this platform".to_string(),
        ))
    }

    /// Prepares the system for USB mass storage mode.
    ///
    /// Performs the necessary cleanup before enabling USB sharing:
    /// - Syncs filesystem buffers
    /// - Drops page caches
    /// - Unmounts /mnt/onboard and /mnt/sd if mounted
    ///
    /// # Errors
    ///
    /// Returns [`UsbError::Partition`] if unmounting fails critically.
    fn prepare_for_usb_share(&self) -> Result<(), UsbError> {
        self.sync_and_drop_caches()?;

        for name in ["onboard", "sd"] {
            let mount_point = format!("/mnt/{}", name);
            if self.is_mounted(&mount_point)? {
                self.unmount_partition(&mount_point)?;
            }
        }

        Ok(())
    }

    /// Remounts partitions after USB sharing is disabled.
    ///
    /// Mounts the onboard partition and optionally the SD card if present.
    /// Uses vfat filesystem with noatime, nodiratime, shortname=mixed, and utf8 options.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError::Partition`] if mounting the onboard partition fails.
    #[cfg(target_os = "linux")]
    fn remount_partitions(&self) -> Result<(), UsbError> {
        let onboard_partition = &self.metadata().partition;

        info!("Remounting onboard partition");

        mount(
            Some(onboard_partition.as_str()),
            "/mnt/onboard",
            Some("vfat"),
            MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME,
            Some("shortname=mixed,utf8"),
        )
        .map_err(|e| {
            error!(error = %e, "Failed to mount onboard partition");
            UsbError::Partition(format!("Cannot mount onboard: {}", e))
        })?;

        info!("Onboard partition remounted successfully");

        let sd_partition = "/dev/mmcblk1p1";
        if Path::new(sd_partition).exists() {
            debug!("Attempting to remount SD card");
            let _ = mount(
                Some(sd_partition),
                "/mnt/sd",
                Some("vfat"),
                MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME,
                Some("shortname=mixed,utf8"),
            );
        }

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn remount_partitions(&self) -> Result<(), UsbError> {
        Err(UsbError::Partition(
            "Mount not supported on this platform".to_string(),
        ))
    }

    /// Runs filesystem check and repair.
    ///
    /// Runs `dosfsck -a -w` on the partition up to twice. The first run
    /// attempts automatic repair. If it exits with a non-zero status
    /// (exit code 1 means recoverable errors were found; see
    /// [`fsck.fat(8)`](https://man7.org/linux/man-pages/man8/fsck.fat.8.html)),
    /// a second run is performed to verify the repairs. Only after both
    /// runs fail is the filesystem considered unrecoverable.
    ///
    /// ## `dosfsck` exit codes
    ///
    /// | Code | Meaning |
    /// |------|---------|
    /// | `0`  | No errors found |
    /// | `1`  | Recoverable errors found (or internal inconsistency) |
    /// | `2`  | Usage error – filesystem was not accessed |
    ///
    /// The two-pass approach mirrors the original shell script behaviour:
    /// the first pass repairs, and the second pass confirms the result.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError::Partition`] if `/mnt/onboard` is still mounted,
    /// to prevent running dosfsck on a live filesystem.
    /// Returns [`UsbError::Filesystem`] if filesystem corruption is detected
    /// and cannot be repaired. Returns [`UsbError::Io`] if the command fails
    /// to execute.
    fn check_filesystem(&self) -> Result<(), UsbError> {
        if self.is_mounted("/mnt/onboard")? {
            error!("Refusing to run filesystem check: /mnt/onboard is still mounted");
            return Err(UsbError::Partition(
                "/mnt/onboard is still mounted; filesystem check aborted".to_string(),
            ));
        }

        let partition = &self.metadata().partition;

        info!(partition = %partition, "Running filesystem check");

        let result = Command::new("dosfsck")
            .args(["-a", "-w", partition])
            .output();

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if output.status.success() {
                    info!("Filesystem check passed");
                    return Ok(());
                }

                warn!(stdout = %stdout, stderr = %stderr, "First filesystem check failed, retrying");

                let retry = Command::new("dosfsck")
                    .args(["-a", "-w", partition])
                    .output();

                match retry {
                    Ok(retry_output) if retry_output.status.success() => {
                        info!("Filesystem check passed on retry");
                        Ok(())
                    }
                    Ok(_) => {
                        error!("Filesystem corruption detected and cannot be repaired");
                        Err(UsbError::Filesystem(
                            "Filesystem corruption detected, reboot required".to_string(),
                        ))
                    }
                    Err(e) => Err(UsbError::Io(e)),
                }
            }
            Err(e) => Err(UsbError::Io(e)),
        }
    }
}
