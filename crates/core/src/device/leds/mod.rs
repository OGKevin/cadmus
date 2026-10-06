//! Device LED control.
//!
//! Provides the [`DeviceLeds`] hardware trait and the [`StatusLed`] command
//! arbiter used by [`Inhibitor`](crate::device::inhibitor::Inhibitor) for
//! the startup pulse, soft-indicate, and Full-inhibit patterns.

mod error;
mod manager;
mod priority;
mod status_led;

pub use error::LedsError;
pub use manager::{DeviceLeds, NoopLeds};
pub use priority::LedPriority;
pub use status_led::{LedPattern, StatusLed, StatusLedGuard};
