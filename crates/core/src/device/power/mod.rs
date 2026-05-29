//! Power management submodule.

mod error;
mod manager;

#[cfg(not(feature = "kobo"))]
mod stub;

#[cfg(feature = "kobo")]
mod kobo;

pub use error::PowerError;
pub use manager::PowerManager;

#[cfg(feature = "kobo")]
pub(crate) use kobo::create_power_manager;

#[cfg(not(feature = "kobo"))]
pub(crate) use stub::create_power_manager;

#[cfg(all(test, not(feature = "kobo")))]
mod tests {
    use super::*;
    use crate::device::Model;

    #[test]
    #[should_panic(expected = "There is no implementation for suspending on this build.")]
    fn test_power_manager_stub_suspend_panics() {
        let manager = create_power_manager(Model::Sage).expect("failed to create power manager");

        let _ = manager.suspend();
    }

    #[test]
    #[should_panic(expected = "There is no implementation for resuming on this build.")]
    fn test_power_manager_stub_resume_panics() {
        let manager = create_power_manager(Model::Sage).expect("failed to create power manager");

        let _ = manager.resume();
    }
}
