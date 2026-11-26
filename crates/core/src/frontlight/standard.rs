// TODO(OGKevin): this shall also be in device and devce/kobo
use super::{Frontlight, LightLevel, LightLevels};
use anyhow::Error;
use nix::ioctl_write_int_bad;
use std::fs::File;
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;

ioctl_write_int_bad!(write_frontlight_intensity, 241);
cfg_select! {
    test => {


const FRONTLIGHT_INTERFACE: &str = "/dev/null";
    }
    _ => {

const FRONTLIGHT_INTERFACE: &str = "/dev/ntx_io";
    }
}

pub struct StandardFrontlight {
    value: LightLevel,
    interface: File,
}

impl StandardFrontlight {
    pub fn new(value: LightLevel) -> Result<StandardFrontlight, Error> {
        let interface = OpenOptions::new().write(true).open(FRONTLIGHT_INTERFACE)?;
        Ok(StandardFrontlight { value, interface })
    }
}

#[async_trait::async_trait]
impl Frontlight for StandardFrontlight {
    /// # SAFETY
    /// The duplicated `/dev/ntx_io` descriptor is owned by the blocking closure,
    /// so it remains valid until the ioctl returns, even if the caller stops
    /// waiting. The ioctl request code and integer payload match the kernel
    /// frontlight brightness interface expected by `write_frontlight_intensity`.
    async fn set_intensity(&mut self, value: LightLevel) -> Result<(), Error> {
        let file = self.interface.try_clone()?;
        crate::runtime::spawn_blocking(move || unsafe {
            write_frontlight_intensity(file.as_raw_fd(), libc::c_int::from(value))
        })
        .await
        .map_err(Error::new)??;
        self.value = value;
        Ok(())
    }

    async fn set_warmth(&mut self, _value: LightLevel) -> Result<(), Error> {
        Ok(())
    }

    fn levels(&self) -> LightLevels {
        LightLevels {
            intensity: self.value,
            warmth: LightLevel::off(),
        }
    }
}
