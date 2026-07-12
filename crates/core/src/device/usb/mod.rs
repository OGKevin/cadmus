//! USB mass storage gadget management.

mod error;
mod manager;

pub(crate) use error::UsbError;
pub use manager::UsbManager;
