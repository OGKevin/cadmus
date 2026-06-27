//! USB mass storage gadget management.

mod error;
mod manager;
#[cfg(any(test, not(feature = "kobo")))]
mod stub;

cfg_select! {
    any(feature = "kobo", docsrs) => {
        mod kobo;
        pub use kobo::KoboUsbManager;
    }
    _ => {}
}

pub(crate) use error::UsbError;
pub use manager::UsbManager;
#[cfg(any(test, not(feature = "kobo")))]
pub use stub::StubUsbManager;
