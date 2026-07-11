use crate::db::Database;
use crate::device::Device;
use crate::device::rtc::AlarmManager;
use crate::dictionary::{Dictionary, load_dictionary_from_db};
use crate::font::Fonts;
use crate::framebuffer::{Display, Framebuffer};
use crate::frontlight::Frontlight as _;
use crate::geom::Rectangle;
use crate::helpers::{Fingerprint, Fp, IsHidden, load_json};
use crate::library::Library;
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

pub struct Context<D: Device> {
    pub device: D,
    pub alarm_manager: Option<AlarmManager<D::Rtc>>,
    pub display: Display,
    pub settings: Settings,
    pub library: Library,
    pub database: Database,
    pub fonts: Fonts,
    pub dictionaries: BTreeMap<String, Dictionary>,
    pub keyboard_layouts: BTreeMap<String, Layout>,
    pub input_history: FxHashMap<ViewId, VecDeque<String>>,
    pub notification_index: u8,
    pub kb_rect: Rectangle,
    pub rng: Xoroshiro128Plus,
    pub plugged: bool,
    pub covered: bool,
    pub shared: bool,
    pub online: bool,
}

impl<D: Device> Context<D> {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn new(
        mut device: D,
        library: Library,
        database: Database,
        settings: Settings,
        fonts: Fonts,
    ) -> Context<D> {
        device.refresh_framebuffer_from_kernel();
        let fb = device.framebuffer();
        let fb_rotation = fb.rotation();
        let dims = fb.dims();
        let rotation = device.transformed_rotation(fb_rotation);
        tracing::trace!(
            fb_rotation,
            transformed_rotation = rotation,
            dims = ?dims,
            "Context::new framebuffer state"
        );
        let rng = Xoroshiro128Plus::seed_from_u64(Local::now().timestamp_subsec_nanos() as u64);
        let alarm_manager = match device.rtc() {
            Ok(rtc) => Some(AlarmManager::new(rtc)),
            Err(e) => {
                tracing::warn!(error = %e, "RTC init failed, alarm manager unavailable");
                None
            }
        };
        Context {
            device,
            alarm_manager,
            display: Display { dims, rotation },
            library,
            database,
            settings,
            fonts,
            dictionaries: BTreeMap::new(),
            keyboard_layouts: BTreeMap::new(),
            input_history: FxHashMap::default(),
            notification_index: 0,
            kb_rect: Rectangle::default(),
            rng,
            plugged: false,
            covered: false,
            shared: false,
            online: false,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub fn load_keyboard_layouts(&mut self) {
        let glob = Glob::new("**/*.json").unwrap().compile_matcher();

        #[cfg(test)]
        let path = Path::new(
            &env::var("TEST_ROOT_DIR")
                .expect("TEST_ROOT_DIR must be set for test using keyboard layouts"),
        )
        .join(KEYBOARD_LAYOUTS_DIRNAME);

        #[cfg(not(test))]
        let path = self.device.install_path(KEYBOARD_LAYOUTS_DIRNAME);

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
        let path = self.device.data_path(DICTIONARIES_DIRNAME);

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
            if let Err(error) = self.device.frontlight_mut().set_warmth(levels.warmth) {
                tracing::error!(error = %error, "failed to set frontlight warmth");
            }
            if let Err(error) = self.device.frontlight_mut().set_intensity(levels.intensity) {
                tracing::error!(error = %error, "failed to set frontlight intensity");
            }
            self.settings.frontlight_levels = levels;
        } else {
            self.settings.frontlight_levels = self.device.frontlight().levels();
            if let Err(error) = self.device.frontlight_mut().turn_off() {
                tracing::error!(error = %error, "failed to turn off frontlight");
            }
        }
    }

    pub fn framebuffer_with_dpi(&mut self) -> (&mut dyn Framebuffer, u16) {
        let dpi = self.device.dpi();
        let fb = self.device.framebuffer_mut();
        (fb, dpi)
    }

    pub fn framebuffer_and_fonts(&mut self) -> (&mut dyn Framebuffer, &mut Fonts, u16) {
        let dpi = self.device.dpi();
        let Context { device, fonts, .. } = self;
        let fb = device.framebuffer_mut();
        (fb, fonts, dpi)
    }

    /// Sets rotation at runtime using the same index for framebuffer and display.
    ///
    /// Writes `rotation` to the framebuffer and sets [`Display::rotation`](crate::framebuffer::Display::rotation)
    /// to the same value. [`crate::device::DeviceRotation::transformed_rotation`] applies only when
    /// reading the boot framebuffer state in [`Self::new`], not on runtime writes.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub fn set_rotation(&mut self, rotation: i8) -> anyhow::Result<(u32, u32)> {
        let fb_rotation = self.device.framebuffer().rotation();
        let fb_dims = self.device.framebuffer().dims();
        let result = self.device.framebuffer_mut().set_rotation(rotation);
        if let Ok(new_dims) = result {
            self.device.refresh_framebuffer_from_kernel();
            self.display.rotation = rotation;
            self.display.dims = new_dims;
            tracing::trace!(
                rotation,
                fb_rotation_before = fb_rotation,
                fb_dims_before = ?fb_dims,
                context_rotation = self.display.rotation,
                context_dims = ?self.display.dims,
                new_fb_dims = ?new_dims,
                fb_rotation_after = self.device.framebuffer().rotation(),
                "set_rotation"
            );
            Ok(new_dims)
        } else {
            result
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
    use crate::battery::Battery as _;
    use crate::db::Database;
    use crate::device::test_device::TestDevice;
    use crate::device::{AppContext, DeviceHardware as _};
    use crate::frontlight::LightLevels;

    pub fn create_test_context() -> AppContext {
        create_test_context_from_device(TestDevice::new())
    }

    pub fn create_test_context_from_device(device: TestDevice) -> AppContext {
        let mut database = Database::new(":memory:").expect("failed to create in-memory database");
        let mut settings = Settings::default();
        database
            .init(&device, 0, &mut settings)
            .expect("failed to run migrations");
        Context::new(
            device,
            Library::new(Path::new("/tmp"), &database, "test").unwrap(),
            database,
            settings,
            Fonts::load_from(
                Path::new(
                    &env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for this test."),
                )
                .to_path_buf(),
            )
            .expect("Failed to load fonts"),
        )
    }

    #[test]
    fn test_create_test_context_defaults() {
        let context = create_test_context();
        assert_eq!(context.display.dims, (600, 800));
        assert!(!context.plugged);
        assert!(!context.covered);
        assert!(!context.shared);
        assert!(!context.online);
        assert_eq!(context.notification_index, 0);
        assert!(context.dictionaries.is_empty());
        assert!(context.keyboard_layouts.is_empty());
        assert!(context.input_history.is_empty());
        assert_eq!(context.kb_rect, Rectangle::default());
    }

    #[test]
    fn test_create_test_context_frontlight() {
        let mut context = create_test_context();
        let levels = context.device.frontlight().levels();
        assert_eq!(levels.intensity, LightLevels::default().intensity);
        assert_eq!(levels.warmth, LightLevels::default().warmth);

        context.set_frontlight(false);
        assert!(!context.settings.frontlight);

        context.set_frontlight(true);
        assert!(context.settings.frontlight);
    }

    #[test]
    fn test_create_test_context_battery() {
        let mut context = create_test_context();
        let capacity = context
            .device
            .battery_mut()
            .capacity()
            .expect("battery capacity");
        assert_eq!(capacity, vec![50.0]);
    }

    #[test]
    fn test_create_test_context_record_input() {
        let mut context = create_test_context();
        context.record_input("hello", ViewId::SearchBar);
        context.record_input("world", ViewId::SearchBar);

        let history = context.input_history.get(&ViewId::SearchBar).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history.front(), Some(&"world".to_string()));
    }

    #[test]
    fn set_rotation_updates_display_rotation() {
        let mut context = create_test_context();
        context.set_rotation(1).expect("rotation should succeed");
        assert_eq!(context.display.rotation, 1);
        assert_eq!(context.device.framebuffer().rotation(), 0);
    }
}
