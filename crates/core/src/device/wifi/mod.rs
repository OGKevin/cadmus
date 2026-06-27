//! WiFi management.

mod error;
mod manager;
#[cfg(any(test, not(feature = "kobo")))]
mod stub;

cfg_select! {
    any(feature = "kobo", docsrs) => {
        mod kobo;
        pub use kobo::KoboWifiManager;
    }
    _ => {}
}

pub use error::WifiError;
pub use manager::WifiManager;
#[cfg(any(test, not(feature = "kobo")))]
pub use stub::StubWifiManager;
