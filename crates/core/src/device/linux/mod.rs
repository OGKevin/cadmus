//! Linux-specific device implementations.

#[cfg(any(feature = "kobo", docsrs))]
pub(crate) mod mounts;
mod rtc;
pub mod soft_suspend;
#[cfg(any(feature = "kobo", docsrs))]
mod time;
#[cfg(any(feature = "kobo", docsrs))]
pub(crate) mod wake_telemetry;

pub use rtc::LinuxRtc;
#[cfg(any(feature = "kobo", docsrs))]
pub use time::set_system_timezone;
