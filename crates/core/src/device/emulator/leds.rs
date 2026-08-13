use crate::device::leds::{DeviceLeds, LedsError};

pub struct EmulatorLeds;

impl DeviceLeds for EmulatorLeds {
    fn on(&self) -> Result<(), LedsError> {
        Ok(())
    }

    fn off(&self) -> Result<(), LedsError> {
        Ok(())
    }
}
