//! Linux-specific device implementations.

#[cfg(any(feature = "kobo", docsrs))]
pub(crate) mod loopback;
mod rtc;
pub mod soft_suspend;
mod time;

pub use rtc::LinuxRtc;
#[cfg(any(feature = "kobo", docsrs))]
pub use time::set_system_timezone;
