//! Linux-specific device implementations.

mod rtc;
mod time;

pub use rtc::LinuxRtc;
#[cfg(feature = "kobo")]
pub use time::set_system_timezone;
