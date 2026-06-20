use crate::battery::Battery;
use crate::db::Database;
use crate::device::CURRENT_DEVICE;
use crate::dictionary::{Dictionary, load_dictionary_from_db};
use crate::font::Fonts;
use crate::framebuffer::{Display, Framebuffer};
use crate::frontlight::Frontlight;
use crate::geom::Rectangle;
use crate::helpers::{Fingerprint, Fp, IsHidden, load_json};
use crate::library::Library;
use crate::lightsensor::LightSensor;
use crate::rtc::AlarmManager;
use crate::settings::Settings;
use crate::view::ViewId;
use crate::view::keyboard::Layout;
use chrono::Local;
use fxhash::FxHashMap;
use globset::Glob;
use rand_core::SeedableRng;
use rand_xoshiro::Xoroshiro128Plus;
use std::collections::{BTreeMap, VecDeque};
#[cfg(test)]
use std::env;
use std::io;
use std::path::Path;
use tracing::error;

use walkdir::WalkDir;

const KEYBOARD_LAYOUTS_DIRNAME: &str = "keyboard-layouts";
pub(crate) const DICTIONARIES_DIRNAME: &str = "dictionaries";
const INPUT_HISTORY_SIZE: usize = 32;

pub struct Context {
    pub fb: Box<dyn Framebuffer>,
    pub alarm_manager: Option<AlarmManager>,
    pub display: Display,
    pub settings: Settings,
    pub library: Library,
    pub database: Database,
    pub fonts: Fonts,
    pub dictionaries: BTreeMap<String, Dictionary>,
    pub keyboard_layouts: BTreeMap<String, Layout>,
    pub input_history: FxHashMap<ViewId, VecDeque<String>>,
    // TODO(OGKevin): this shall be on the device struct, instead of on context
    pub frontlight: Box<dyn Frontlight>,
    pub battery: Box<dyn Battery>,
    pub lightsensor: Box<dyn LightSensor>,
    pub notification_index: u8,
    pub kb_rect: Rectangle,
    pub rng: Xoroshiro128Plus,
    pub plugged: bool,
    pub covered: bool,
    pub shared: bool,
    pub online: bool,
}

impl Context {
    pub fn new(
        fb: Box<dyn Framebuffer>,
        library: Library,
        database: Database,
        settings: Settings,
        fonts: Fonts,
        battery: Box<dyn Battery>,
        frontlight: Box<dyn Frontlight>,
        lightsensor: Box<dyn LightSensor>,
    ) -> Context {
        let dims = fb.dims();
        let rotation = CURRENT_DEVICE.transformed_rotation(fb.rotation());
        let rng = Xoroshiro128Plus::seed_from_u64(Local::now().timestamp_subsec_nanos() as u64);
        let alarm_manager = match CURRENT_DEVICE.rtc() {
            Ok(rtc) => Some(AlarmManager::new(rtc.clone())),
            Err(e) => {
                tracing::warn!(error = %e, "RTC init failed, alarm manager unavailable");
                None
            }
        };
        Context {
            fb,
            alarm_manager,
            display: Display { dims, rotation },
            library,
            database,
            settings,
            fonts,
            dictionaries: BTreeMap::new(),
            keyboard_layouts: BTreeMap::new(),
            input_history: FxHashMap::default(),
            battery,
            frontlight,
            lightsensor,
            notification_index: 0,
            kb_rect: Rectangle::default(),
            rng,
            plugged: false,
            covered: false,
            shared: false,
            online: false,
        }
    }

    pub fn load_keyboard_layouts(&mut self) {
        let glob = Glob::new("**/*.json").unwrap().compile_matcher();

        #[cfg(test)]
        let path = Path::new(
            &env::var("TEST_ROOT_DIR")
                .expect("TEST_ROOT_DIR must be set for test using keyboard layouts"),
        )
        .join(KEYBOARD_LAYOUTS_DIRNAME);

        #[cfg(not(test))]
        let path = CURRENT_DEVICE.install_path(KEYBOARD_LAYOUTS_DIRNAME);

        for entry in WalkDir::new(path)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !e.is_hidden())
        {
            if entry.is_err() {
                continue;
            }
            let entry = entry.unwrap();
            let path = entry.path();
            if !glob.is_match(path) {
                continue;
            }
            if let Ok(layout) = load_json::<Layout, _>(path)
                .map_err(|e| error!("Can't load {}: {:#?}.", path.display(), e))
            {
                self.keyboard_layouts.insert(layout.name.clone(), layout);
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn load_dictionaries(&mut self) {
        self.dictionaries.clear();

        let glob = Glob::new("**/*.index").unwrap().compile_matcher();

        #[cfg(test)]
        let path = Path::new(
            env::var("TEST_ROOT_DIR")
                .expect("Please set TEST_ROOT_DIR for tests that need dictionaries")
                .as_str(),
        )
        .join(DICTIONARIES_DIRNAME);

        #[cfg(not(test))]
        let path = CURRENT_DEVICE.data_path(DICTIONARIES_DIRNAME);

        for entry in WalkDir::new(path)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !e.is_hidden())
        {
            if entry.is_err() {
                continue;
            }
            let entry = entry.unwrap();
            if !glob.is_match(entry.path()) {
                continue;
            }
            let index_path = entry.path().to_path_buf();
            let mut content_path = index_path.clone();
            content_path.set_extension("dict.dz");
            if !content_path.exists() {
                content_path.set_extension("");
            }

            let dict_result = match fingerprint_dict_pair(&index_path) {
                Ok(fp) => load_dictionary_from_db(&content_path, &self.database, fp),
                Err(e) => {
                    tracing::warn!(
                        path = %index_path.display(),
                        error = %e,
                        "failed to fingerprint index file, skipping dictionary"
                    );
                    continue;
                }
            };

            if let Ok(mut dict) = dict_result {
                let name = dict.short_name().ok().unwrap_or_else(|| {
                    index_path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
                self.dictionaries.insert(name, dict);
            }
        }
    }

    pub fn record_input(&mut self, text: &str, id: ViewId) {
        if text.is_empty() {
            return;
        }

        let history = self.input_history.entry(id).or_insert_with(VecDeque::new);

        if history.front().map(String::as_str) != Some(text) {
            history.push_front(text.to_string());
        }

        if history.len() > INPUT_HISTORY_SIZE {
            history.pop_back();
        }
    }

    /// Enables or disables the device frontlight and keeps the persisted
    /// frontlight settings in sync.
    ///
    /// When automatic frontlight is enabled and coordinates are available,
    /// turning the light on recomputes the effective levels for the current
    /// time before applying them.
    pub fn set_frontlight(&mut self, enable: bool) {
        self.settings.frontlight = enable;

        if enable {
            let levels = if self.settings.auto_frontlight {
                if let Some(coords) = crate::settings::resolve_coordinates(&self.settings) {
                    let night_brightness = self
                        .settings
                        .auto_frontlight_night_brightness
                        .unwrap_or_default();
                    crate::frontlight::auto::compute_auto_frontlight_levels(
                        Local::now(),
                        coords,
                        night_brightness,
                        self.settings.frontlight_levels.intensity,
                    )
                } else {
                    self.settings.frontlight_levels
                }
            } else {
                self.settings.frontlight_levels
            };
            if let Err(error) = self.frontlight.set_warmth(levels.warmth) {
                tracing::error!(error = %error, "failed to set frontlight warmth");
            }
            if let Err(error) = self.frontlight.set_intensity(levels.intensity) {
                tracing::error!(error = %error, "failed to set frontlight intensity");
            }
            self.settings.frontlight_levels = levels;
        } else {
            self.settings.frontlight_levels = self.frontlight.levels();
            if let Err(error) = self.frontlight.turn_off() {
                tracing::error!(error = %error, "failed to turn off frontlight");
            }
        }
    }
}

/// Fingerprints a StarDict dictionary pair by hashing only the `.index` file.
///
/// The `.index` and `.dict` files in a StarDict pair are always installed and
/// replaced together, so hashing the `.index` alone is sufficient to detect
/// any change to either file.
fn fingerprint_dict_pair(index_path: &Path) -> io::Result<Fp> {
    index_path.fingerprint()
}

#[cfg(test)]
pub mod test_helpers {
    use super::*;
    use crate::battery::FakeBattery;
    use crate::db::Database;
    use crate::framebuffer::Pixmap;
    use crate::frontlight::LightLevels;

    pub fn create_test_context() -> Context {
        let mut database = Database::new(":memory:").expect("failed to create in-memory database");
        database.init(0).expect("failed to run migrations");
        Context::new(
            Box::new(Pixmap::new(600, 800, 1)),
            Library::new(Path::new("/tmp"), &database, "test").unwrap(),
            database,
            Settings::default(),
            Fonts::load_from(
                Path::new(
                    &env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for this test."),
                )
                .to_path_buf(),
            )
            .expect("Failed to load fonts"),
            Box::new(FakeBattery::new()),
            Box::new(LightLevels::default()),
            Box::new(0u16),
        )
    }
}
