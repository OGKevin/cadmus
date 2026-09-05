//! Device battery capacity and charge status.
//!
//! Cadmus reads battery state through [`Battery`]: a [`Send`] + [`Sync`] trait
//! whose methods take `&self` so devices can hold a shared backend (typically
//! [`std::sync::Arc`]) and concurrent callers — UI, lifecycle tasks, inhibitor gates —
//! can sample capacity without exclusive access.
//!
//! # Return shape
//!
//! Both [`Battery::capacity`] and [`Battery::status`] return a [`Vec`] whose
//! first element is always the onboard cell. When a SleepCover / power-cover
//! pack is connected ([`KoboBattery`](crate::device::kobo::battery::KoboBattery) on Kobo), a second element reports the
//! auxiliary pack.
//!
//! # Implementations
//!
//! | Type | Use |
//! |------|-----|
//! | [`FakeBattery`] | Emulator, tests, and non-Kobo [`DeviceHardware`](crate::device::DeviceHardware) backends |
//! | [`KoboBattery`](crate::device::kobo::battery::KoboBattery) | Kobo sysfs fuel gauge (feature `kobo`) |
//!
//! # Examples
//!
//! ```
//! use cadmus_core::device::battery::{Battery, FakeBattery, Status};
//! use std::sync::Arc;
//!
//! let battery = Arc::new(FakeBattery::new());
//! assert_eq!(battery.capacity().unwrap(), vec![50.0]);
//! assert_eq!(battery.status().unwrap(), vec![Status::Discharging]);
//! ```

mod fake;

use anyhow::Error;
use std::sync::Arc;

pub use self::fake::FakeBattery;

/// Charge status reported by a battery cell.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Status {
    /// Running on battery power.
    Discharging,
    /// Plugged in and actively charging.
    Charging,
    /// Plugged in and at or near full charge (`Not charging` / `Full` on Kobo).
    Charged,
    /// Unmapped or unreadable status from the backend.
    Unknown,
}

impl Status {
    /// Returns whether external power is connected (charging or full).
    pub fn is_wired(self) -> bool {
        matches!(self, Status::Charging | Status::Charged)
    }
}

/// Shared-readable battery backend.
///
/// Implementations must be safe to share across threads ([`Send`] + [`Sync`])
/// and cheap to call through shared references. Device code stores backends as
/// `Arc<dyn Battery>` or `Arc<FakeBattery>` and exposes them via
/// [`DeviceHardware::battery`](crate::device::DeviceHardware::battery).
pub trait Battery: Send + Sync {
    /// Returns current capacity in percent for each cell: `[main]` or `[main, cover]`.
    fn capacity(&self) -> Result<Vec<f32>, Error>;

    /// Returns charge status for each cell, in the same order as [`Self::capacity`].
    fn status(&self) -> Result<Vec<Status>, Error>;
}

impl<T: Battery + ?Sized> Battery for Box<T> {
    fn capacity(&self) -> Result<Vec<f32>, Error> {
        (**self).capacity()
    }
    fn status(&self) -> Result<Vec<Status>, Error> {
        (**self).status()
    }
}

impl<T: Battery + ?Sized> Battery for Arc<T> {
    fn capacity(&self) -> Result<Vec<f32>, Error> {
        (**self).capacity()
    }
    fn status(&self) -> Result<Vec<Status>, Error> {
        (**self).status()
    }
}
