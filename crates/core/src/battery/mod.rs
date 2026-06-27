mod fake;
#[cfg(any(feature = "kobo", docsrs))]
mod kobo;

use anyhow::Error;

pub use self::fake::FakeBattery;
#[cfg(any(feature = "kobo", docsrs))]
pub use self::kobo::KoboBattery;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Status {
    Discharging,
    Charging,
    Charged,
    Unknown, // Full,
}

impl Status {
    pub fn is_wired(self) -> bool {
        matches!(self, Status::Charging | Status::Charged)
    }
}

pub trait Battery: Send {
    fn capacity(&mut self) -> Result<Vec<f32>, Error>;
    fn status(&mut self) -> Result<Vec<Status>, Error>;
}

impl<T: Battery + ?Sized> Battery for Box<T> {
    fn capacity(&mut self) -> Result<Vec<f32>, Error> {
        (**self).capacity()
    }
    fn status(&mut self) -> Result<Vec<Status>, Error> {
        (**self).status()
    }
}
