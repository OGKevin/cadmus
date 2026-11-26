// TODO(OGkevin): this shall also be under device/ with implementaitons in sub modules
use super::{Frontlight, LightLevel, LightLevels};
use crate::device::Model;
use anyhow::Error;
use rustc_hash::FxHashMap;
use std::fs::File;
use std::fs::OpenOptions;
#[cfg(not(test))]
use std::io::Read;
use std::io::Write;
#[cfg(not(test))]
use std::path::PathBuf;

#[cfg(not(test))]
const FRONTLIGHT_INTERFACE: &str = "/sys/class/backlight";

// Aura ONE
const FRONTLIGHT_WHITE_A: &str = "lm3630a_led1b";
const FRONTLIGHT_RED_A: &str = "lm3630a_led1a";
const FRONTLIGHT_GREEN_A: &str = "lm3630a_ledb";

// Aura H₂O Edition 2
const FRONTLIGHT_WHITE_B: &str = "lm3630a_ledb";
const FRONTLIGHT_ORANGE_B: &str = "lm3630a_leda";

#[cfg(not(test))]
const FRONTLIGHT_VALUE: &str = "brightness";
#[cfg(not(test))]
const FRONTLIGHT_MAX_VALUE: &str = "max_brightness";
#[cfg(not(test))]
const FRONTLIGHT_POWER: &str = "bl_power";

const FRONTLIGHT_POWER_ON: i16 = 31;
const FRONTLIGHT_POWER_OFF: i16 = 0;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum LightColor {
    White,
    Red,
    Green,
    Orange,
}

fn frontlight_dirs(model: Model) -> FxHashMap<LightColor, &'static str> {
    match model {
        #[cfg(feature = "kobo")]
        Model::Kobo(m) => match m {
            crate::device::kobo::Model::AuraONE | crate::device::kobo::Model::AuraONELimEd => [
                (LightColor::White, FRONTLIGHT_WHITE_A),
                (LightColor::Red, FRONTLIGHT_RED_A),
                (LightColor::Green, FRONTLIGHT_GREEN_A),
            ]
            .iter()
            .cloned()
            .collect(),
            _ => [
                (LightColor::White, FRONTLIGHT_WHITE_B),
                (LightColor::Orange, FRONTLIGHT_ORANGE_B),
            ]
            .iter()
            .cloned()
            .collect(),
        },
        #[cfg(feature = "emulator")]
        Model::Emulator => unimplemented!(),
        Model::TestDevice => todo!(),
    }
}

pub struct NaturalFrontlight {
    intensity: LightLevel,
    warmth: LightLevel,
    values: FxHashMap<LightColor, File>,
    powers: FxHashMap<LightColor, File>,
    maxima: FxHashMap<LightColor, i16>,
}

impl NaturalFrontlight {
    pub fn new(
        intensity: LightLevel,
        warmth: LightLevel,
        model: Model,
    ) -> Result<NaturalFrontlight, Error> {
        let mut maxima = FxHashMap::default();
        let mut values = FxHashMap::default();
        let mut powers = FxHashMap::default();
        for (light, name) in frontlight_dirs(model).iter() {
            cfg_select! {
                test => {
                    let _ = name;
                    maxima.insert(*light, 100);
                    values.insert(*light, OpenOptions::new().write(true).open("/dev/null")?);
                    powers.insert(*light, OpenOptions::new().write(true).open("/dev/null")?);
                }
                _ => {
                    let dir = PathBuf::from(FRONTLIGHT_INTERFACE).join(name);
                    let mut buf = String::new();
                    let mut file = File::open(dir.join(FRONTLIGHT_MAX_VALUE))?;
                    file.read_to_string(&mut buf)?;
                    maxima.insert(*light, buf.trim_end().parse()?);
                    let file = OpenOptions::new().write(true).open(dir.join(FRONTLIGHT_VALUE))?;
                    values.insert(*light, file);
                    let file = OpenOptions::new().write(true).open(dir.join(FRONTLIGHT_POWER))?;
                    powers.insert(*light, file);
                }
            }
        }
        Ok(NaturalFrontlight {
            intensity,
            warmth,
            maxima,
            values,
            powers,
        })
    }
}

fn set(
    maxima: &FxHashMap<LightColor, i16>,
    values: &mut FxHashMap<LightColor, File>,
    powers: &mut FxHashMap<LightColor, File>,
    c: LightColor,
    percent: f32,
) -> Result<(), Error> {
    let max_value = maxima[&c] as f32;
    let value = (percent.clamp(0.0, 100.0) / 100.0 * max_value) as i16;
    let mut file = &values[&c];
    write!(file, "{}", value)?;
    let mut file = &powers[&c];
    let power = if value > 0 {
        FRONTLIGHT_POWER_ON
    } else {
        FRONTLIGHT_POWER_OFF
    };
    write!(file, "{}", power)?;
    Ok(())
}

fn update_levels(
    maxima: &FxHashMap<LightColor, i16>,
    values: &mut FxHashMap<LightColor, File>,
    powers: &mut FxHashMap<LightColor, File>,
    intensity: LightLevel,
    warmth: LightLevel,
) -> Result<(), Error> {
    let i = intensity.as_fraction();
    let w = warmth.as_fraction();
    let white = 80.0 * i * (1.0 - w).sqrt();
    set(maxima, values, powers, LightColor::White, white)?;

    if values.len() == 3 {
        let green = 64.0 * (w * i).sqrt();
        let red = if green == 0.0 {
            0.0
        } else {
            green + 20.0 + 7.0 * (1.0 - green / 64.0) + w * 4.0
        };
        set(maxima, values, powers, LightColor::Red, red)?;
        set(maxima, values, powers, LightColor::Green, green)?;
    } else {
        let orange = 95.0 * (w * i).sqrt();
        set(maxima, values, powers, LightColor::Orange, orange)?;
    }
    Ok(())
}

impl NaturalFrontlight {
    async fn update(&mut self, intensity: LightLevel, warmth: LightLevel) -> Result<(), Error> {
        let values = self
            .values
            .iter()
            .map(|(light, file)| Ok((*light, file.try_clone()?)))
            .collect::<Result<FxHashMap<LightColor, File>, std::io::Error>>()?;
        let powers = self
            .powers
            .iter()
            .map(|(light, file)| Ok((*light, file.try_clone()?)))
            .collect::<Result<FxHashMap<LightColor, File>, std::io::Error>>()?;
        let maxima = self.maxima.clone();
        crate::runtime::spawn_blocking(move || {
            let mut values = values;
            let mut powers = powers;
            update_levels(&maxima, &mut values, &mut powers, intensity, warmth)
        })
        .await
        .map_err(Error::new)??;
        self.intensity = intensity;
        self.warmth = warmth;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Frontlight for NaturalFrontlight {
    async fn set_intensity(&mut self, value: LightLevel) -> Result<(), Error> {
        let warmth = self.warmth;
        self.update(value, warmth).await
    }

    async fn set_warmth(&mut self, value: LightLevel) -> Result<(), Error> {
        let intensity = self.intensity;
        self.update(intensity, value).await
    }

    fn levels(&self) -> LightLevels {
        LightLevels {
            intensity: self.intensity,
            warmth: self.warmth,
        }
    }
}
