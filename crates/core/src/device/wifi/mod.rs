//! WiFi management.

mod error;
mod manager;
mod network_info;
mod session;

pub use error::WifiError;
#[cfg(any(
    test,
    docsrs,
    all(
        feature = "deviceless",
        not(any(feature = "kobo", feature = "emulator"))
    )
))]
pub(crate) use error::clone_wifi_error;
pub use manager::WifiManager;
pub use network_info::{Essid, NetworkInfo};
pub use session::{DEFAULT_ACQUIRE_TIMEOUT, WifiLease, WifiSession, WifiSessionError};
