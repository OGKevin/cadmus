//! USB Manager trait definition.

use crate::device::usb::error::UsbError;

/// Trait for USB mass storage gadget management.
///
/// This trait abstracts over platform-specific implementations that enable
/// USB mass storage mode for exposing device storage to a host computer.
///
/// # Lifecycle
///
/// 1. Call [`enable`](UsbManager::enable) when the user connects to a host
///    and wants to share storage.
/// 2. The implementation unmounts the onboard partition, configures the USB
///    gadget, and exposes the storage to the host.
/// 3. Call [`disable`](UsbManager::disable) when the user disconnects.
/// 4. The implementation disables the gadget, runs filesystem checks, and
///    remounts the partition.
///
/// # Example
///
/// ```ignore
/// use cadmus_core::device::usb::{UsbManager, UsbError};
///
/// # fn example(usb_manager: &dyn UsbManager) -> Result<(), UsbError> {
/// // Enable USB sharing
/// usb_manager.enable()?;
///
/// // ... device is now in USB mass storage mode ...
///
/// // Disable USB sharing
/// usb_manager.disable()?;
/// # Ok(())
/// # }
/// ```
pub trait UsbManager: Send + Sync {
    /// Enables USB mass storage mode.
    ///
    /// This method performs the following operations:
    ///
    /// 1. Syncs filesystem buffers and drops caches
    /// 2. Unmounts `/mnt/onboard` (and `/mnt/sd` if present)
    /// 3. Configures and enables the USB mass storage gadget
    ///
    /// After this call returns successfully, the Kobo's internal storage is
    /// accessible to the connected USB host.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError`] if any step fails:
    /// - [`UsbError::Partition`] if unmounting fails
    /// - [`UsbError::GadgetConfig`] if USB gadget configuration fails
    /// - [`UsbError::KernelModule`] if module loading fails
    ///
    /// # Example
    ///
    /// ```ignore
    /// use cadmus_core::device::usb::UsbManager;
    ///
    /// # fn example(usb_manager: &dyn UsbManager) -> Result<(), cadmus_core::device::usb::UsbError> {
    /// usb_manager.enable()?;
    /// # Ok(())
    /// # }
    /// ```
    fn enable(&self) -> Result<(), UsbError>;

    /// Disables USB mass storage mode.
    ///
    /// This method performs the following operations:
    ///
    /// 1. Disables the USB gadget
    /// 2. Tears down the USB gadget configuration
    /// 3. Runs filesystem checks with `dosfsck`
    /// 4. Remounts `/mnt/onboard` (and `/mnt/sd` if present)
    ///
    /// If filesystem corruption is detected and cannot be repaired, this
    /// method may trigger a reboot by returning an error.
    ///
    /// # Errors
    ///
    /// Returns [`UsbError`] if any step fails:
    /// - [`UsbError::KernelModule`] if module unloading fails
    /// - [`UsbError::Filesystem`] if filesystem check/repair fails
    /// - [`UsbError::Partition`] if remounting fails
    ///
    /// # Example
    ///
    /// ```ignore
    /// use cadmus_core::device::usb::UsbManager;
    ///
    /// # fn example(usb_manager: &dyn UsbManager) -> Result<(), cadmus_core::device::usb::UsbError> {
    /// usb_manager.disable()?;
    /// # Ok(())
    /// # }
    /// ```
    fn disable(&self) -> Result<(), UsbError>;
}

impl<T: UsbManager + ?Sized> UsbManager for Box<T> {
    fn enable(&self) -> Result<(), UsbError> {
        (**self).enable()
    }
    fn disable(&self) -> Result<(), UsbError> {
        (**self).disable()
    }
}
