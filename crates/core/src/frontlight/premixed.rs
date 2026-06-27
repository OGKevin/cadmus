use super::{Frontlight, LightLevel, LightLevels};
use anyhow::Error;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[cfg(not(test))]
const FRONTLIGHT_WHITE: &str = "/sys/class/backlight/mxc_msp430.0/brightness";
#[cfg(test)]
const FRONTLIGHT_WHITE: &str = "/dev/null";

// Forma
#[cfg(not(test))]
const FRONTLIGHT_ORANGE_A: &str = "/sys/class/backlight/tlc5947_bl/color";
#[cfg(test)]
const FRONTLIGHT_ORANGE_A: &str = "/dev/null";
// Libra H₂O, Clara HD, Libra 2, Clara BW, Libra Colour, Clara Colour
#[cfg(not(test))]
const FRONTLIGHT_ORANGE_B: &str = "/sys/class/backlight/lm3630a_led/color";
#[cfg(test)]
const FRONTLIGHT_ORANGE_B: &str = "/dev/null";
// Sage, Libra 2, Clara 2E, Elipsa 2E
#[cfg(not(test))]
const FRONTLIGHT_ORANGE_C: &str = "/sys/class/leds/aw99703-bl_FL1/color";
#[cfg(test)]
const FRONTLIGHT_ORANGE_C: &str = "/dev/null";

pub struct PremixedFrontlight {
    intensity: LightLevel,
    warmth: LightLevel,
    white: File,
    orange: File,
    mark: u8,
}

impl PremixedFrontlight {
    pub fn new(
        intensity: LightLevel,
        warmth: LightLevel,
        mark: u8,
    ) -> Result<PremixedFrontlight, Error> {
        let white = OpenOptions::new().write(true).open(FRONTLIGHT_WHITE)?;
        let orange_path = if Path::new(FRONTLIGHT_ORANGE_C).exists() {
            FRONTLIGHT_ORANGE_C
        } else if Path::new(FRONTLIGHT_ORANGE_B).exists() {
            FRONTLIGHT_ORANGE_B
        } else {
            FRONTLIGHT_ORANGE_A
        };
        let orange = OpenOptions::new().write(true).open(orange_path)?;
        Ok(PremixedFrontlight {
            intensity,
            warmth,
            white,
            orange,
            mark,
        })
    }
}

impl Frontlight for PremixedFrontlight {
    fn set_intensity(&mut self, intensity: LightLevel) -> Result<(), Error> {
        write!(self.white, "{}", i16::from(intensity))?;
        self.intensity = intensity;
        Ok(())
    }

    fn set_warmth(&mut self, warmth: LightLevel) -> Result<(), Error> {
        if self.mark != 8 {
            write!(self.orange, "{}", warmth.as_10_base_inverted())?;
        } else {
            write!(self.orange, "{}", warmth.as_10_base())?;
        }
        self.warmth = warmth;
        Ok(())
    }

    fn levels(&self) -> LightLevels {
        LightLevels {
            intensity: self.intensity,
            warmth: self.warmth,
        }
    }
}
