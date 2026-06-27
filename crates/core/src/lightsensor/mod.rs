mod kobo;

use anyhow::Error;

pub use self::kobo::KoboLightSensor;

pub trait LightSensor: Send {
    fn level(&mut self) -> Result<u16, Error>;
}

impl<T: LightSensor + ?Sized> LightSensor for Box<T> {
    fn level(&mut self) -> Result<u16, Error> {
        (**self).level()
    }
}

impl LightSensor for u16 {
    fn level(&mut self) -> Result<u16, Error> {
        Ok(*self)
    }
}
