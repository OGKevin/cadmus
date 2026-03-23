//! WiFi management.

mod error;
mod manager;
#[cfg(not(feature = "kobo"))]
mod stub;

#[cfg(feature = "kobo")]
mod kobo;

pub use error::WifiError;
pub use manager::WifiManager;

#[cfg(feature = "kobo")]
pub(crate) use kobo::create_wifi_manager;

#[cfg(not(feature = "kobo"))]
pub(crate) use stub::create_wifi_manager;
