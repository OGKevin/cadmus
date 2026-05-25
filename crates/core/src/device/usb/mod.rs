//! USB mass storage gadget management.

mod error;
mod manager;
#[cfg(not(feature = "kobo"))]
mod stub;

#[cfg(feature = "kobo")]
mod kobo;

pub(crate) use error::UsbError;
pub use manager::UsbManager;

#[cfg(feature = "kobo")]
pub(crate) use kobo::create_usb_manager;

#[cfg(not(feature = "kobo"))]
pub(crate) use stub::StubUsbManager;
