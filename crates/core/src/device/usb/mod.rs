//! USB mass storage gadget management.

mod error;
mod manager;

cfg_select! {
    feature = "kobo" => {
        mod kobo;
        pub(crate) use kobo::create_usb_manager;
    }
    _ => {
        mod stub;
        pub(crate) use stub::StubUsbManager;
    }
}

pub(crate) use error::UsbError;
pub use manager::UsbManager;
