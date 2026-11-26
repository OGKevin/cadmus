use crate::device::power::{PowerError, PowerManager};

pub struct EmulatorPowerManager;

impl PowerManager for EmulatorPowerManager {
    async fn suspend(&self) -> Result<(), PowerError> {
        unimplemented!("Power management not available in emulator")
    }

    async fn resume(&self) -> Result<(), PowerError> {
        unimplemented!("Power management not available in emulator")
    }
}
