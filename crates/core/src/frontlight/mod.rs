/// Automatic frontlight calculations based on sunrise and sunset.
pub mod auto;
#[cfg(any(feature = "kobo", docsrs))]
mod natural;
#[cfg(any(feature = "kobo", docsrs))]
mod premixed;
#[cfg(any(feature = "kobo", docsrs))]
mod standard;

#[cfg(any(feature = "kobo", docsrs))]
pub use self::natural::NaturalFrontlight;
#[cfg(any(feature = "kobo", docsrs))]
pub use self::premixed::PremixedFrontlight;
#[cfg(any(feature = "kobo", docsrs))]
pub use self::standard::StandardFrontlight;
use crate::geom::lerp;
use libc::c_int;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt::{Display, Formatter};

/// The level of light intensity from 0-100.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub struct LightLevel(f32);

/// The default implementation reutrns 1%, so that the screen can be seen
/// if the device happens to be turned on in a dark place.
impl Default for LightLevel {
    fn default() -> Self {
        Self(1f32)
    }
}

impl From<LightLevel> for c_int {
    fn from(value: LightLevel) -> Self {
        value.0.round() as c_int
    }
}

impl From<LightLevel> for i16 {
    fn from(value: LightLevel) -> Self {
        value.0.round() as i16
    }
}

impl From<LightLevel> for String {
    fn from(value: LightLevel) -> Self {
        format!("{:.0}", value.0)
    }
}

impl Display for LightLevel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.0}%", self.value())
    }
}

impl std::cmp::PartialEq<f32> for LightLevel {
    fn eq(&self, other: &f32) -> bool {
        self.value().eq(other)
    }
}

impl std::cmp::PartialOrd<f32> for LightLevel {
    fn partial_cmp(&self, other: &f32) -> Option<Ordering> {
        self.value().partial_cmp(other)
    }
}

impl std::ops::Sub<f32> for LightLevel {
    type Output = LightLevel;

    fn sub(self, rhs: f32) -> Self::Output {
        Self(self.value() - rhs)
    }
}
impl From<f32> for LightLevel {
    fn from(value: f32) -> Self {
        Self(value.clamp(0.0, 100.0))
    }
}

impl From<LightLevel> for f32 {
    fn from(value: LightLevel) -> Self {
        value.0
    }
}

impl LightLevel {
    const ZERO: f32 = 0.0;

    /// Returns the absolute value of the light level.
    pub fn abs(self) -> Self {
        self.value().abs().into()
    }

    /// Returns a light level representing `0%` output.
    pub fn off() -> Self {
        Self(Self::ZERO)
    }

    fn value(&self) -> f32 {
        self.0
    }

    /// Given a value between 0.0 and 1, this normalizes LightLevel accordingly to 0-100.
    pub fn from_fraction(fraction: f32) -> Self {
        Self::from(fraction * 100f32)
    }

    /// Returns the level as a value between 0.0-1
    pub fn as_fraction(&self) -> f32 {
        self.value() / 100.0
    }

    /// Instead of 0-100, this returns a value between 0-10.
    /// Usefull for e.g. `/sys/class/backlight/lm3630a_led/color` that accepts
    /// a value between 0-10.
    pub fn as_10_base(&self) -> i16 {
        (self.value() / 10.0).round() as i16
    }

    /// Similar to [`Self::as_10_base`] but instead of 10 being max, 0 is max.
    pub fn as_10_base_inverted(&self) -> i16 {
        10 - self.as_10_base()
    }
}

/// A complete frontlight state containing brightness and warmth.
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct LightLevels {
    /// The overall frontlight brightness.
    pub intensity: LightLevel,
    /// The warmth of the emitted light.
    pub warmth: LightLevel,
}

impl Default for LightLevels {
    fn default() -> Self {
        LightLevels {
            intensity: LightLevel::default(),
            warmth: LightLevel::default(),
        }
    }
}

impl LightLevels {
    /// Linearly interpolates between two frontlight states.
    ///
    /// `t = 0.0` returns `self` and `t = 1.0` returns `other`.
    pub fn interpolate(self, other: Self, t: f32) -> Self {
        LightLevels {
            intensity: LightLevel(lerp(self.intensity.value(), other.intensity.value(), t)),
            warmth: LightLevel(lerp(self.warmth.value(), other.warmth.value(), t)),
        }
    }
}

pub trait Frontlight: Send {
    // value is a percentage.
    fn set_intensity(&mut self, value: LightLevel) -> anyhow::Result<()>;
    fn set_warmth(&mut self, value: LightLevel) -> anyhow::Result<()>;
    fn levels(&self) -> LightLevels;
    /// Turns the FrontLight off by setting everything to [`LightLevel::off()`]
    fn turn_off(&mut self) -> anyhow::Result<()> {
        self.set_intensity(LightLevel::off())?;
        self.set_warmth(LightLevel::off())?;
        Ok(())
    }
}

impl<T: Frontlight + ?Sized> Frontlight for Box<T> {
    fn set_intensity(&mut self, value: LightLevel) -> anyhow::Result<()> {
        (**self).set_intensity(value)
    }
    fn set_warmth(&mut self, value: LightLevel) -> anyhow::Result<()> {
        (**self).set_warmth(value)
    }
    fn levels(&self) -> LightLevels {
        (**self).levels()
    }
}

impl Frontlight for LightLevels {
    fn set_intensity(&mut self, value: LightLevel) -> anyhow::Result<()> {
        self.intensity = value;
        Ok(())
    }

    fn set_warmth(&mut self, value: LightLevel) -> anyhow::Result<()> {
        self.warmth = value;
        Ok(())
    }

    fn levels(&self) -> LightLevels {
        *self
    }
}
