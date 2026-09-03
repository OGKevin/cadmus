//! In-memory battery for emulator and unit tests.
//!
//! [`FakeBattery`] stores capacity and status behind mutexes so
//! [`Battery::capacity`] and [`Battery::status`] take `&self` and work when the
//! instance is wrapped in [`std::sync::Arc`]. Tests adjust capacity with
//! [`FakeBattery::set_capacity`] without needing `&mut`.

use super::{Battery, Status};
use anyhow::Error;
use std::sync::Mutex;

/// Configurable single-cell battery for emulator and test devices.
///
/// Defaults to 50% capacity and [`Status::Discharging`]. Share via [`std::sync::Arc`] and
/// read through [`Battery`]; use [`Self::set_capacity`] to simulate drain or
/// charging in tests.
pub struct FakeBattery {
    capacity: Mutex<f32>,
    status: Mutex<Status>,
}

impl Default for FakeBattery {
    fn default() -> Self {
        Self {
            capacity: Mutex::new(50.0),
            status: Mutex::new(Status::Discharging),
        }
    }
}

impl FakeBattery {
    /// Creates a battery at the default capacity and status.
    pub fn new() -> FakeBattery {
        Self::default()
    }

    /// Sets the reported capacity percent (visible to subsequent shared reads).
    pub fn set_capacity(&self, capacity: f32) {
        *self.capacity.lock().unwrap_or_else(|e| e.into_inner()) = capacity;
    }
}

impl Battery for FakeBattery {
    fn capacity(&self) -> Result<Vec<f32>, Error> {
        Ok(vec![
            *self.capacity.lock().unwrap_or_else(|e| e.into_inner()),
        ])
    }

    fn status(&self) -> Result<Vec<Status>, Error> {
        Ok(vec![*self.status.lock().unwrap_or_else(|e| e.into_inner())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn shared_read_returns_default_capacity() {
        let battery = FakeBattery::new();
        assert_eq!(battery.capacity().unwrap(), vec![50.0]);
        assert_eq!(battery.status().unwrap(), vec![Status::Discharging]);
    }

    #[test]
    fn set_capacity_visible_through_shared_read() {
        let battery = FakeBattery::new();
        battery.set_capacity(12.0);
        assert_eq!(battery.capacity().unwrap(), vec![12.0]);
    }

    #[test]
    fn arc_shared_capacity_read() {
        let battery = Arc::new(FakeBattery::new());
        let cloned = Arc::clone(&battery);
        assert_eq!(cloned.capacity().unwrap(), vec![50.0]);
    }
}
