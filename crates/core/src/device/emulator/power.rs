use crate::device::power::{PowerError, PowerManager};

pub struct EmulatorPowerManager;

impl PowerManager for EmulatorPowerManager {
    fn suspend(&self) -> Result<(), PowerError> {
        unimplemented!("Power management not available in emulator")
    }

    fn resume(&self) -> Result<(), PowerError> {
        unimplemented!("Power management not available in emulator")
    }
}
