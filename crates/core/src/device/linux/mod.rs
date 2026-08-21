//! Linux-specific device implementations.

mod rtc;
pub mod soft_suspend;
mod time;

pub use rtc::LinuxRtc;
#[cfg(any(feature = "kobo", docsrs))]
pub use time::set_system_timezone;
