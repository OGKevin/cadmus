//! WiFi management.

mod error;
mod manager;

cfg_select! {
    feature = "kobo" => {
        mod kobo;
        pub(crate) use kobo::create_wifi_manager;
    }
    _ => {
        mod stub;
        pub(crate) use stub::create_wifi_manager;
    }
}

pub use error::WifiError;
pub use manager::WifiManager;
