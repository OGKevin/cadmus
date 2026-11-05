#![allow(unused)]

use crate::geom::lerp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Color {
    Gray(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    pub fn gray(&self) -> u8 {
        match *self {
            Color::Gray(level) => level,
            Color::Rgb(red, green, blue) => {
                (red as f32 * 0.2126 + green as f32 * 0.7152 + blue as f32 * 0.0722) as u8
            }
        }
    }

    pub fn rgb(&self) -> [u8; 3] {
        match *self {
            Color::Gray(level) => [level; 3],
            Color::Rgb(red, green, blue) => [red, green, blue],
        }
    }

    pub fn from_rgb(rgb: &[u8]) -> Color {
        Color::Rgb(rgb[0], rgb[1], rgb[2])
    }

    /// Parses `#rrggbb` or `rrggbb`.
    ///
    /// Pure white and pure black map to [`WHITE`] and [`BLACK`], the palette
    /// entries the device draws them as, so settings round-trip unchanged.
    pub fn from_hex(hex: &str) -> Option<Color> {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(match (red, green, blue) {
            (255, 255, 255) => WHITE,
            (0, 0, 0) => BLACK,
            _ => Color::Rgb(red, green, blue),
        })
    }

    pub fn apply<F>(&self, f: F) -> Color
    where
        F: Fn(u8) -> u8,
    {
        match *self {
            Color::Gray(level) => Color::Gray(f(level)),
            Color::Rgb(red, green, blue) => Color::Rgb(f(red), f(green), f(blue)),
        }
    }

    pub fn lerp(&self, color: Color, alpha: f32) -> Color {
        match (*self, color) {
            (Color::Gray(l1), Color::Gray(l2)) => {
                Color::Gray(lerp(l1 as f32, l2 as f32, alpha) as u8)
            }
            (Color::Rgb(red, green, blue), Color::Gray(level)) => Color::Rgb(
                lerp(red as f32, level as f32, alpha) as u8,
                lerp(green as f32, level as f32, alpha) as u8,
                lerp(blue as f32, level as f32, alpha) as u8,
            ),
            (Color::Gray(level), Color::Rgb(red, green, blue)) => Color::Rgb(
                lerp(level as f32, red as f32, alpha) as u8,
                lerp(level as f32, green as f32, alpha) as u8,
                lerp(level as f32, blue as f32, alpha) as u8,
            ),
            (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => Color::Rgb(
                lerp(r1 as f32, r2 as f32, alpha) as u8,
                lerp(g1 as f32, g2 as f32, alpha) as u8,
                lerp(b1 as f32, b2 as f32, alpha) as u8,
            ),
        }
    }

    pub fn invert(&mut self) {
        match self {
            Color::Gray(level) => *level = 255 - *level,
            Color::Rgb(red, green, blue) => {
                *red = 255 - *red;
                *green = 255 - *green;
                *blue = 255 - *blue;
            }
        }
    }

    pub fn shift(&mut self, drift: u8) {
        match self {
            Color::Gray(level) => *level = level.saturating_sub(drift),
            Color::Rgb(red, green, blue) => {
                *red = red.saturating_sub(drift);
                *green = green.saturating_sub(drift);
                *blue = blue.saturating_sub(drift);
            }
        }
    }
}

macro_rules! gray {
    ($a:expr) => {
        $crate::color::Color::Gray($a)
    };
}

pub const GRAY00: Color = gray!(0x00);
pub const GRAY01: Color = gray!(0x11);
pub const GRAY02: Color = gray!(0x22);
pub const GRAY03: Color = gray!(0x33);
pub const GRAY04: Color = gray!(0x44);
pub const GRAY05: Color = gray!(0x55);
pub const GRAY06: Color = gray!(0x66);
pub const GRAY07: Color = gray!(0x77);
pub const GRAY08: Color = gray!(0x88);
pub const GRAY09: Color = gray!(0x99);
pub const GRAY10: Color = gray!(0xAA);
pub const GRAY11: Color = gray!(0xBB);
pub const GRAY12: Color = gray!(0xCC);
pub const GRAY13: Color = gray!(0xDD);
pub const GRAY14: Color = gray!(0xEE);
pub const GRAY15: Color = gray!(0xFF);

pub const BLACK: Color = GRAY00;
pub const WHITE: Color = GRAY15;

pub const TEXT_NORMAL: [Color; 3] = [WHITE, BLACK, GRAY08];
pub const TEXT_BUMP_SMALL: [Color; 3] = [GRAY14, BLACK, GRAY07];
pub const TEXT_BUMP_LARGE: [Color; 3] = [GRAY11, BLACK, BLACK];

pub const TEXT_INVERTED_SOFT: [Color; 3] = [GRAY05, WHITE, WHITE];
pub const TEXT_INVERTED_HARD: [Color; 3] = [BLACK, WHITE, GRAY06];

pub const SEPARATOR_NORMAL: Color = GRAY10;
pub const SEPARATOR_STRONG: Color = GRAY07;

pub const KEYBOARD_BG: Color = GRAY12;
pub const BATTERY_FILL: Color = GRAY12;
pub const READING_PROGRESS: Color = GRAY07;

pub const PROGRESS_FULL: Color = GRAY05;
pub const PROGRESS_EMPTY: Color = GRAY13;
pub const PROGRESS_VALUE: Color = GRAY06;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hex_parses_palette_rgb_and_rejects_invalid() {
        assert_eq!(Color::from_hex("#ffffff"), Some(WHITE));
        assert_eq!(Color::from_hex("000000"), Some(BLACK));
        assert_eq!(Color::from_hex("#804020"), Some(Color::Rgb(128, 64, 32)));
        assert_eq!(Color::from_hex("gggggg"), None);
        assert_eq!(Color::from_hex("#fff"), None);
        assert_eq!(Color::from_hex(""), None);
    }
}
