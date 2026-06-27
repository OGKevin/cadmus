use super::{Battery, Status};
use anyhow::Error;

pub struct FakeBattery {
    capacity: f32,
    status: Status,
}

impl Default for FakeBattery {
    fn default() -> Self {
        Self {
            capacity: 50.0,
            status: Status::Discharging,
        }
    }
}

impl FakeBattery {
    pub fn new() -> FakeBattery {
        Self::default()
    }

    pub fn set_capacity(&mut self, capacity: f32) {
        self.capacity = capacity;
    }
}

impl Battery for FakeBattery {
    fn capacity(&mut self) -> Result<Vec<f32>, Error> {
        Ok(vec![self.capacity])
    }

    fn status(&mut self) -> Result<Vec<Status>, Error> {
        Ok(vec![self.status])
    }
}
