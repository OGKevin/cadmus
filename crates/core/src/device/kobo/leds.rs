//! Kobo status LED via sysfs brightness.
//!
//! Turns the LED on or off by writing `"1"` or `"0"` to a model-resolved
//! brightness path under `/sys/class/leds/`. [`KoboLeds::for_model`] asks
//! [`super::Model::led_brightness_path`]: known models (for example Libra
//! Colour) return a hardcoded path; others probe
//! [`LED_BRIGHTNESS_CANDIDATES`] and info-log the hit so new hardcodes can be
//! crowd-sourced from device logs. If no path is found, writes target the
//! default `LED` node and are skipped when that path is missing.

use crate::device::kobo::Model;
use crate::device::leds::{DeviceLeds, LedsError};
use std::fs;
use std::path::{Path, PathBuf};

/// Default brightness path used when discovery finds nothing (graceful no-op).
pub(super) const LED_BRIGHTNESS_PATH: &str = "/sys/class/leds/LED/brightness";

/// Known standard-LED brightness nodes, same order as `contrib/cadmus.sh`.
pub(super) const LED_BRIGHTNESS_CANDIDATES: &[&str] = &[
    LED_BRIGHTNESS_PATH,
    "/sys/class/leds/GLED/brightness",
    "/sys/class/leds/bd71828-green-led/brightness",
];

/// Returns the first candidate path that exists on the filesystem.
pub(super) fn discover_led_brightness_path(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(Path::new)
        .find(|path| path.exists())
        .map(Path::to_path_buf)
}

/// Kobo status LED controller writing a resolved sysfs brightness path.
pub struct KoboLeds {
    brightness_path: PathBuf,
}

impl KoboLeds {
    /// Creates a controller for `model` using [`Model::led_brightness_path`].
    ///
    /// Falls back to [`LED_BRIGHTNESS_PATH`] when no path is known or
    /// discovered so missing-path writes stay graceful.
    pub fn for_model(model: Model) -> Self {
        let path = model
            .led_brightness_path()
            .unwrap_or_else(|| PathBuf::from(LED_BRIGHTNESS_PATH));
        Self::with_path(path)
    }

    /// Creates a controller targeting the default Kobo LED brightness path.
    pub fn new() -> Self {
        Self {
            brightness_path: PathBuf::from(LED_BRIGHTNESS_PATH),
        }
    }

    /// Creates a controller targeting `brightness_path` (used by unit tests).
    pub fn with_path(brightness_path: impl Into<PathBuf>) -> Self {
        Self {
            brightness_path: brightness_path.into(),
        }
    }

    fn write_brightness(&self, value: &str) -> Result<(), LedsError> {
        let path = self.brightness_path.as_path();
        if !Path::new(path).exists() {
            tracing::warn!(path = %path.display(), "LED brightness path missing");
            return Ok(());
        }

        tracing::debug!(path = %path.display(), value, "Writing LED brightness");
        fs::write(path, value).map_err(|e| {
            tracing::error!(error = %e, path = %path.display(), "Failed to write LED brightness");
            LedsError::Io(e)
        })
    }
}

impl Default for KoboLeds {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceLeds for KoboLeds {
    fn on(&self) -> Result<(), LedsError> {
        self.write_brightness("1")
    }

    fn off(&self) -> Result<(), LedsError> {
        self.write_brightness("0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_writes_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("brightness");
        fs::write(&path, "0").expect("seed");
        let leds = KoboLeds::with_path(&path);

        leds.on().expect("on");

        assert_eq!(fs::read_to_string(&path).expect("read").trim(), "1");
    }

    #[test]
    fn off_writes_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("brightness");
        fs::write(&path, "1").expect("seed");
        let leds = KoboLeds::with_path(&path);

        leds.off().expect("off");

        assert_eq!(fs::read_to_string(&path).expect("read").trim(), "0");
    }

    #[test]
    fn missing_path_is_graceful() {
        let leds = KoboLeds::with_path("/nonexistent/leds/LED/brightness");

        assert!(leds.on().is_ok());
        assert!(leds.off().is_ok());
    }

    #[test]
    fn discover_returns_first_existing_candidate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gled = dir.path().join("GLED");
        fs::create_dir_all(&gled).expect("mkdir");
        let brightness = gled.join("brightness");
        fs::write(&brightness, "0").expect("seed");

        let missing = dir.path().join("LED/brightness");
        let candidates = [missing.to_str().unwrap(), brightness.to_str().unwrap()];

        assert_eq!(discover_led_brightness_path(&candidates), Some(brightness));
    }

    #[test]
    fn discover_returns_none_when_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("LED/brightness");
        let b = dir.path().join("GLED/brightness");
        let candidates = [a.to_str().unwrap(), b.to_str().unwrap()];

        assert_eq!(discover_led_brightness_path(&candidates), None);
    }
}
