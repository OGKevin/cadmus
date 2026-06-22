//! Setting kinds for the General category.

use super::{
    InputSettingKind, SettingData, SettingIdentity, SettingKind, ToggleSettings, WidgetKind,
};
use crate::device::CURRENT_DEVICE;
use crate::fl;
use crate::frontlight::LightLevel;
use crate::geolocation::Coordinates;
use crate::i18n::I18nDisplay;
use crate::settings::{Settings, StartupMode};
use crate::view::{Bus, EntryId, EntryKind, Event, ToggleEvent, ViewId};
use anyhow::Error;
use std::fs;

/// Language and locale selection setting
pub struct Locale;

impl SettingKind for Locale {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::Locale
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-language")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let current = settings.locale.as_ref().map(|l| l.to_string());
        let display = current
            .as_deref()
            .unwrap_or(crate::i18n::DEFAULT_LOCALE)
            .to_string();

        let entries = crate::i18n::AVAILABLE_LOCALES
            .iter()
            .map(|&tag| {
                let lang_id: Option<unic_langid::LanguageIdentifier> = tag.parse().ok();
                EntryKind::RadioButton(
                    tag.to_string(),
                    EntryId::SetLocale(lang_id),
                    current.as_deref() == Some(tag),
                )
            })
            .collect::<Vec<_>>();

        SettingData {
            value: display,
            widget: WidgetKind::SubMenu(entries),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::SetLocale(locale)) = evt {
            settings.locale = locale.clone();
            crate::i18n::init(locale.as_ref());
            let display = locale
                .as_ref()
                .map(|l| l.to_string())
                .unwrap_or_else(|| crate::i18n::DEFAULT_LOCALE.to_string());
            return (Some(display), true);
        }
        (None, false)
    }
}

/// Keyboard layout selection setting
pub struct KeyboardLayout;

impl SettingKind for KeyboardLayout {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::KeyboardLayout
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-keyboard-layout")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let current_layout = settings.keyboard_layout.clone();
        let available_layouts = get_available_layouts().unwrap_or_default();

        let entries = available_layouts
            .iter()
            .map(|layout| {
                EntryKind::RadioButton(
                    layout.clone(),
                    EntryId::SetKeyboardLayout(layout.clone()),
                    current_layout == *layout,
                )
            })
            .collect::<Vec<_>>();

        SettingData {
            value: current_layout,
            widget: WidgetKind::SubMenu(entries),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::SetKeyboardLayout(layout)) = evt {
            settings.keyboard_layout = layout.clone();
            return (Some(layout.clone()), true);
        }
        (None, false)
    }
}

/// Auto suspend timeout setting
pub struct AutoSuspend;

impl SettingKind for AutoSuspend {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutoSuspend
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-auto-suspend")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let value = if settings.auto_suspend == 0.0 {
            fl!("settings-general-never")
        } else {
            format!("{:.1}", settings.auto_suspend)
        };

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditAutoSuspend)),
        }
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        Some(self)
    }
}

impl InputSettingKind for AutoSuspend {
    fn submit_view_id(&self) -> ViewId {
        ViewId::AutoSuspendInput
    }

    fn open_entry_id(&self) -> EntryId {
        EntryId::EditAutoSuspend
    }

    fn input_label(&self) -> String {
        fl!("settings-general-auto-suspend-input")
    }

    fn input_max_chars(&self) -> usize {
        10
    }

    fn current_text(&self, settings: &Settings) -> String {
        if settings.auto_suspend == 0.0 {
            "0".to_string()
        } else {
            format!("{:.1}", settings.auto_suspend)
        }
    }

    fn apply_text(&self, text: &str, settings: &mut Settings) -> String {
        if let Ok(value) = text.parse::<f32>() {
            settings.auto_suspend = value;
        }
        if settings.auto_suspend == 0.0 {
            fl!("settings-general-never")
        } else {
            format!("{:.1}", settings.auto_suspend)
        }
    }
}

/// Auto power off timeout setting
pub struct AutoPowerOff;

impl SettingKind for AutoPowerOff {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutoPowerOff
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-auto-power-off")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let value = if settings.auto_power_off == 0.0 {
            fl!("settings-general-never")
        } else {
            format!("{:.1}", settings.auto_power_off)
        };

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditAutoPowerOff)),
        }
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        Some(self)
    }
}

impl InputSettingKind for AutoPowerOff {
    fn submit_view_id(&self) -> ViewId {
        ViewId::AutoPowerOffInput
    }

    fn open_entry_id(&self) -> EntryId {
        EntryId::EditAutoPowerOff
    }

    fn input_label(&self) -> String {
        fl!("settings-general-auto-power-off-input")
    }

    fn input_max_chars(&self) -> usize {
        10
    }

    fn current_text(&self, settings: &Settings) -> String {
        if settings.auto_power_off == 0.0 {
            "0".to_string()
        } else {
            format!("{:.1}", settings.auto_power_off)
        }
    }

    fn apply_text(&self, text: &str, settings: &mut Settings) -> String {
        if let Ok(value) = text.parse::<f32>() {
            settings.auto_power_off = value;
        }
        if settings.auto_power_off == 0.0 {
            fl!("settings-general-never")
        } else {
            format!("{:.1}", settings.auto_power_off)
        }
    }
}

/// Sleep cover enable/disable toggle setting
pub struct SleepCover;

impl SettingKind for SleepCover {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::SleepCover
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-enable-sleep-cover")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.sleep_cover.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.sleep_cover,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::SleepCover)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::SleepCover)) = evt {
            settings.sleep_cover = !settings.sleep_cover;
            return (Some(settings.sleep_cover.to_string()), true);
        }
        (None, false)
    }
}

/// Auto share enable/disable toggle setting
pub struct AutoShare;

impl SettingKind for AutoShare {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutoShare
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-enable-auto-share")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.auto_share.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.auto_share,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::AutoShare)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::AutoShare)) = evt {
            settings.auto_share = !settings.auto_share;
            return (Some(settings.auto_share.to_string()), true);
        }
        (None, false)
    }
}

/// Auto time sync enable/disable toggle setting
pub struct AutoTime;

impl SettingKind for AutoTime {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutoTime
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-auto-time")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.auto_time.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.auto_time,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::AutoTime)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::AutoTime)) = evt {
            settings.auto_time = !settings.auto_time;
            return (Some(settings.auto_time.to_string()), true);
        }
        (None, false)
    }
}

/// Button scheme (natural/inverted) toggle setting
pub struct ButtonScheme;

impl SettingKind for ButtonScheme {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::ButtonScheme
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-button-scheme")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let enabled = settings.button_scheme == crate::settings::ButtonScheme::Natural;
        SettingData {
            value: settings.button_scheme.to_i18n_string(),
            widget: WidgetKind::Toggle {
                left_label: crate::settings::ButtonScheme::Natural.to_i18n_string(),
                right_label: crate::settings::ButtonScheme::Inverted.to_i18n_string(),
                enabled,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::ButtonScheme)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        bus: &mut Bus,
    ) -> (Option<String>, bool) {
        let new_scheme = match evt {
            Event::Toggle(ToggleEvent::Setting(ToggleSettings::ButtonScheme)) => {
                match settings.button_scheme {
                    crate::settings::ButtonScheme::Natural => {
                        Some(crate::settings::ButtonScheme::Inverted)
                    }
                    crate::settings::ButtonScheme::Inverted => {
                        Some(crate::settings::ButtonScheme::Natural)
                    }
                }
            }
            Event::Select(EntryId::SetButtonScheme(scheme)) => Some(*scheme),
            _ => None,
        };

        if let Some(scheme) = new_scheme {
            settings.button_scheme = scheme;
            bus.push_back(Event::Select(EntryId::SetButtonScheme(scheme)));
            return (Some(settings.button_scheme.to_i18n_string()), true);
        }
        (None, false)
    }
}

/// Setting kind for toggling automatic frontlight adjustment.
///
/// Changing this setting triggers a re-evaluation of the active frontlight
/// configuration so the device can immediately react to the new mode.
pub struct AutoFrontlight;

impl SettingKind for AutoFrontlight {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutoFrontlight
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-auto-frontlight")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.auto_frontlight.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.auto_frontlight,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::AutoFrontlight)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::AutoFrontlight)) = evt {
            settings.auto_frontlight = !settings.auto_frontlight;
            bus.push_back(Event::AutoFrontlightConfigChanged);
            return (Some(settings.auto_frontlight.to_string()), true);
        }
        (None, false)
    }
}

/// Setting kind for configuring the brightness used while the sun is down.
///
/// This value is applied by automatic frontlight mode after sunset and before
/// sunrise.
pub struct AutoFrontlightBrightness;

impl SettingKind for AutoFrontlightBrightness {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutoFrontlightBrightness
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-auto-frontlight-brightness")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: self.display_value(settings),
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditAutoFrontlightBrightness)),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Submit(ViewId::AutoFrontlightBrightnessInput, text) = evt {
            let display = self.apply_text(text, settings);
            bus.push_back(Event::AutoFrontlightConfigChanged);
            return (Some(display), true);
        }

        (None, false)
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        Some(self)
    }
}

impl AutoFrontlightBrightness {
    fn display_value(&self, settings: &Settings) -> String {
        settings
            .auto_frontlight_night_brightness
            .map(|brightness| brightness.to_string())
            .unwrap_or_else(|| LightLevel::default().to_string())
    }
}

impl InputSettingKind for AutoFrontlightBrightness {
    fn submit_view_id(&self) -> ViewId {
        ViewId::AutoFrontlightBrightnessInput
    }

    fn open_entry_id(&self) -> EntryId {
        EntryId::EditAutoFrontlightBrightness
    }

    fn input_label(&self) -> String {
        fl!("settings-general-auto-frontlight-brightness-input")
    }

    fn input_max_chars(&self) -> usize {
        3
    }

    fn current_text(&self, settings: &Settings) -> String {
        settings
            .auto_frontlight_night_brightness
            .map(|b| b.into())
            .unwrap_or_else(|| LightLevel::default().into())
    }

    fn apply_text(&self, text: &str, settings: &mut Settings) -> String {
        if let Ok(value) = text.trim().parse::<f32>() {
            settings.auto_frontlight_night_brightness = Some(value.into());
        }
        self.display_value(settings)
    }
}

/// Setting kind for overriding automatic frontlight coordinates manually.
///
/// Users can enter a `latitude, longitude` pair to control which sunrise and
/// sunset times automatic frontlight should follow.
pub struct AutoFrontlightManualCoordinates;

impl SettingKind for AutoFrontlightManualCoordinates {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutoFrontlightManualCoordinates
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-auto-frontlight-manual-coordinates")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: self.display_value(settings),
            widget: WidgetKind::ActionLabel(Event::Select(
                EntryId::EditAutoFrontlightManualCoordinates,
            )),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Submit(ViewId::AutoFrontlightManualCoordinatesInput, text) = evt {
            let display = self.apply_text(text, settings);
            bus.push_back(Event::AutoFrontlightConfigChanged);
            return (Some(display), true);
        }

        (None, false)
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        Some(self)
    }
}

impl AutoFrontlightManualCoordinates {
    fn display_value(&self, settings: &Settings) -> String {
        settings
            .auto_frontlight_manual_coordinates
            .map(|coordinates| {
                format!(
                    "{:.4}, {:.4}",
                    coordinates.latitude(),
                    coordinates.longitude()
                )
            })
            .unwrap_or_else(|| fl!("settings-general-not-set"))
    }
}

impl InputSettingKind for AutoFrontlightManualCoordinates {
    fn submit_view_id(&self) -> ViewId {
        ViewId::AutoFrontlightManualCoordinatesInput
    }

    fn open_entry_id(&self) -> EntryId {
        EntryId::EditAutoFrontlightManualCoordinates
    }

    fn input_label(&self) -> String {
        fl!("settings-general-auto-frontlight-manual-coordinates-input")
    }

    fn input_max_chars(&self) -> usize {
        32
    }

    fn current_text(&self, settings: &Settings) -> String {
        settings
            .auto_frontlight_manual_coordinates
            .map(|coordinates| coordinates.to_string())
            .unwrap_or_default()
    }

    fn apply_text(&self, text: &str, settings: &mut Settings) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            settings.auto_frontlight_manual_coordinates = None;
            return self.display_value(settings);
        }

        let mut parts = trimmed.split(',').map(str::trim);
        let parsed_coordinates =
            parts
                .next()
                .zip(parts.next())
                .and_then(|(latitude, longitude)| {
                    let latitude = latitude.parse::<f64>().ok()?;
                    let longitude = longitude.parse::<f64>().ok()?;
                    Coordinates::new(latitude, longitude).ok()
                });

        if parsed_coordinates.is_some() && parts.next().is_none() {
            settings.auto_frontlight_manual_coordinates = parsed_coordinates;
        }

        self.display_value(settings)
    }
}

/// Settings retention count setting
pub struct SettingsRetention;

impl SettingKind for SettingsRetention {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::SettingsRetention
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-settings-retention")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.settings_retention.to_string(),
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditSettingsRetention)),
        }
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        Some(self)
    }
}

impl InputSettingKind for SettingsRetention {
    fn submit_view_id(&self) -> ViewId {
        ViewId::SettingsRetentionInput
    }

    fn open_entry_id(&self) -> EntryId {
        EntryId::EditSettingsRetention
    }

    fn input_label(&self) -> String {
        fl!("settings-general-settings-retention")
    }

    fn input_max_chars(&self) -> usize {
        3
    }

    fn current_text(&self, settings: &Settings) -> String {
        settings.settings_retention.to_string()
    }

    fn apply_text(&self, text: &str, settings: &mut Settings) -> String {
        if let Ok(value) = text.parse::<usize>() {
            settings.settings_retention = value;
        }
        settings.settings_retention.to_string()
    }
}

/// Database backup retention count setting
pub struct DbBackupRetentionSetting;

impl SettingKind for DbBackupRetentionSetting {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::DbBackupRetention
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-db-backup-retention")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.db_backup_retention.to_string(),
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditDbBackupRetention)),
        }
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        Some(self)
    }
}

impl InputSettingKind for DbBackupRetentionSetting {
    fn submit_view_id(&self) -> ViewId {
        ViewId::DbBackupRetentionInput
    }

    fn open_entry_id(&self) -> EntryId {
        EntryId::EditDbBackupRetention
    }

    fn input_label(&self) -> String {
        fl!("settings-general-db-backup-retention-input")
    }

    fn input_max_chars(&self) -> usize {
        3
    }

    fn current_text(&self, settings: &Settings) -> String {
        settings.db_backup_retention.to_string()
    }

    fn apply_text(&self, text: &str, settings: &mut Settings) -> String {
        if let Ok(value) = text.parse::<usize>() {
            settings.db_backup_retention = value;
        }
        settings.db_backup_retention.to_string()
    }
}

/// Startup mode selection setting (Home or Last File)
pub struct StartupModeSetting;

impl SettingKind for StartupModeSetting {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::StartupMode
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-general-startup-mode")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let current = settings.startup_mode;
        let entries = vec![
            EntryKind::RadioButton(
                StartupMode::Home.to_i18n_string(),
                EntryId::SetStartupMode(StartupMode::Home),
                current == StartupMode::Home,
            ),
            EntryKind::RadioButton(
                StartupMode::LastFile.to_i18n_string(),
                EntryId::SetStartupMode(StartupMode::LastFile),
                current == StartupMode::LastFile,
            ),
        ];

        SettingData {
            value: current.to_i18n_string(),
            widget: WidgetKind::SubMenu(entries),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::SetStartupMode(mode)) = evt {
            settings.startup_mode = *mode;
            return (Some(mode.to_i18n_string()), true);
        }
        (None, false)
    }
}

/// Scans the keyboard-layouts directory for available keyboard layouts
fn get_available_layouts() -> Result<Vec<String>, Error> {
    let layouts_dir = CURRENT_DEVICE.install_path("keyboard-layouts");
    let mut layouts = Vec::new();

    if layouts_dir.exists() {
        for entry in fs::read_dir(&layouts_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let layout_name = stem
                        .chars()
                        .enumerate()
                        .map(|(i, c)| {
                            if i == 0 {
                                c.to_uppercase().collect::<String>()
                            } else {
                                c.to_string()
                            }
                        })
                        .collect::<String>();
                    layouts.push(layout_name);
                }
            }
        }
    }

    layouts.sort();
    Ok(layouts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::view::settings_editor::kinds::{InputSettingKind, ToggleSettings};
    use crate::view::{Bus, EntryId, Event, ToggleEvent};
    use std::collections::VecDeque;

    mod locale {
        use super::*;

        #[test]
        fn handle_set_locale_updates_settings() {
            let setting = Locale;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let locale: Option<unic_langid::LanguageIdentifier> = Some("de-DE".parse().unwrap());
            let event = Event::Select(EntryId::SetLocale(locale.clone()));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(result.0.unwrap(), "de-DE");
            assert_eq!(settings.locale, locale);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = Locale;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }

    mod keyboard_layout {
        use super::*;

        #[test]
        fn handle_set_layout_updates_settings() {
            let setting = KeyboardLayout;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetKeyboardLayout("German".to_string()));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(result.0.unwrap(), "German");
            assert_eq!(settings.keyboard_layout, "German");
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = KeyboardLayout;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }

    mod auto_suspend {
        use super::*;

        #[test]
        fn apply_text_parses_and_updates() {
            let setting = AutoSuspend;
            let mut settings = Settings::default();

            let display = setting.apply_text("60.0", &mut settings);

            assert_eq!(display, "60.0");
            assert_eq!(settings.auto_suspend, 60.0);
        }

        #[test]
        fn apply_text_returns_never_for_zero() {
            let setting = AutoSuspend;
            let mut settings = Settings::default();

            let display = setting.apply_text("0", &mut settings);

            assert_eq!(display, "Never");
            assert_eq!(settings.auto_suspend, 0.0);
        }

        #[test]
        fn apply_text_ignores_invalid_input() {
            let setting = AutoSuspend;
            let mut settings = Settings {
                auto_suspend: 30.0,
                ..Default::default()
            };

            let display = setting.apply_text("invalid", &mut settings);

            assert_eq!(settings.auto_suspend, 30.0);
            assert_eq!(display, "30.0");
        }
    }

    mod auto_power_off {
        use super::*;

        #[test]
        fn apply_text_parses_and_updates() {
            let setting = AutoPowerOff;
            let mut settings = Settings::default();

            let display = setting.apply_text("14.0", &mut settings);

            assert_eq!(display, "14.0");
            assert_eq!(settings.auto_power_off, 14.0);
        }

        #[test]
        fn apply_text_returns_never_for_zero() {
            let setting = AutoPowerOff;
            let mut settings = Settings::default();

            let display = setting.apply_text("0", &mut settings);

            assert_eq!(display, "Never");
            assert_eq!(settings.auto_power_off, 0.0);
        }

        #[test]
        fn apply_text_ignores_invalid_input() {
            let setting = AutoPowerOff;
            let mut settings = Settings {
                auto_power_off: 7.0,
                ..Default::default()
            };

            let display = setting.apply_text("invalid", &mut settings);

            assert_eq!(settings.auto_power_off, 7.0);
            assert_eq!(display, "7.0");
        }
    }

    mod sleep_cover {
        use super::*;

        #[test]
        fn handle_toggle_event_toggles_value() {
            let setting = SleepCover;
            let mut settings = Settings {
                sleep_cover: true,
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::SleepCover));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(result.0.unwrap(), "false");
            assert!(!settings.sleep_cover);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = SleepCover;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }

    mod auto_share {
        use super::*;

        #[test]
        fn handle_toggle_event_toggles_value() {
            let setting = AutoShare;
            let mut settings = Settings {
                auto_share: false,
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::AutoShare));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(result.0.unwrap(), "true");
            assert!(settings.auto_share);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = AutoShare;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }

    mod auto_frontlight {
        use super::*;

        #[test]
        fn brightness_apply_text_parses_and_updates() {
            let setting = AutoFrontlightBrightness;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let display = setting.apply_text("25", &mut settings);
            let result = setting.handle(
                &Event::Submit(ViewId::AutoFrontlightBrightnessInput, "25".to_string()),
                &mut settings,
                &mut bus,
            );

            assert_eq!(display, "25%");
            assert_eq!(settings.auto_frontlight_night_brightness, Some(25.0.into()));
            assert_eq!(result, (Some("25%".to_string()), true));
            assert!(matches!(
                bus.pop_front(),
                Some(Event::AutoFrontlightConfigChanged)
            ));
        }

        #[test]
        fn brightness_apply_text_ignores_invalid_input() {
            let setting = AutoFrontlightBrightness;
            let mut settings = Settings {
                auto_frontlight_night_brightness: Some(10.0.into()),
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();

            let display = setting.apply_text("invalid", &mut settings);
            let result = setting.handle(
                &Event::Submit(ViewId::AutoFrontlightBrightnessInput, "invalid".to_string()),
                &mut settings,
                &mut bus,
            );

            assert_eq!(display, "10%");
            assert_eq!(settings.auto_frontlight_night_brightness, Some(10.0.into()));
            assert_eq!(result, (Some("10%".to_string()), true));
            assert!(matches!(
                bus.pop_front(),
                Some(Event::AutoFrontlightConfigChanged)
            ));
        }

        #[test]
        fn manual_coordinates_apply_text_parses_and_updates() {
            let setting = AutoFrontlightManualCoordinates;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let display = setting.apply_text("51.5074, -0.1278", &mut settings);
            let result = setting.handle(
                &Event::Submit(
                    ViewId::AutoFrontlightManualCoordinatesInput,
                    "51.5074, -0.1278".to_string(),
                ),
                &mut settings,
                &mut bus,
            );

            assert_eq!(display, "51.5074, -0.1278");
            assert_eq!(
                settings.auto_frontlight_manual_coordinates,
                Some(Coordinates::new(51.5074, -0.1278).unwrap())
            );
            assert_eq!(result, (Some("51.5074, -0.1278".to_string()), true));
            assert!(matches!(
                bus.pop_front(),
                Some(Event::AutoFrontlightConfigChanged)
            ));
        }

        #[test]
        fn manual_coordinates_apply_text_clears_on_empty_input() {
            let setting = AutoFrontlightManualCoordinates;
            let mut settings = Settings {
                auto_frontlight_manual_coordinates: Some(
                    Coordinates::new(51.5074, -0.1278).unwrap(),
                ),
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();

            let display = setting.apply_text("", &mut settings);
            let result = setting.handle(
                &Event::Submit(ViewId::AutoFrontlightManualCoordinatesInput, "".to_string()),
                &mut settings,
                &mut bus,
            );

            assert_eq!(display, "Not set");
            assert_eq!(settings.auto_frontlight_manual_coordinates, None);
            assert_eq!(result, (Some("Not set".to_string()), true));
            assert!(matches!(
                bus.pop_front(),
                Some(Event::AutoFrontlightConfigChanged)
            ));
        }

        #[test]
        fn manual_coordinates_apply_text_ignores_invalid_input() {
            let setting = AutoFrontlightManualCoordinates;
            let mut settings = Settings {
                auto_frontlight_manual_coordinates: Some(
                    Coordinates::new(51.5074, -0.1278).unwrap(),
                ),
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();

            let display = setting.apply_text("invalid", &mut settings);
            let result = setting.handle(
                &Event::Submit(
                    ViewId::AutoFrontlightManualCoordinatesInput,
                    "invalid".to_string(),
                ),
                &mut settings,
                &mut bus,
            );

            assert_eq!(display, "51.5074, -0.1278");
            assert_eq!(
                settings.auto_frontlight_manual_coordinates,
                Some(Coordinates::new(51.5074, -0.1278).unwrap())
            );
            assert_eq!(result, (Some("51.5074, -0.1278".to_string()), true));
            assert!(matches!(
                bus.pop_front(),
                Some(Event::AutoFrontlightConfigChanged)
            ));
        }
    }

    mod button_scheme {
        use super::*;
        use crate::settings::ButtonScheme;

        #[test]
        fn handle_toggle_event_switches_natural_to_inverted() {
            let setting = ButtonScheme;
            let mut settings = Settings {
                button_scheme: ButtonScheme::Natural,
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::ButtonScheme));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert_eq!(settings.button_scheme, ButtonScheme::Inverted);
            assert_eq!(bus.len(), 1);
            assert!(result.0.is_some());
        }

        #[test]
        fn handle_toggle_event_switches_inverted_to_natural() {
            let setting = ButtonScheme;
            let mut settings = Settings {
                button_scheme: ButtonScheme::Inverted,
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::ButtonScheme));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert_eq!(settings.button_scheme, ButtonScheme::Natural);
            assert_eq!(bus.len(), 1);
            assert!(result.0.is_some());
        }

        #[test]
        fn handle_set_scheme_event_applies_directly() {
            let setting = ButtonScheme;
            let mut settings = Settings {
                button_scheme: ButtonScheme::Natural,
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetButtonScheme(ButtonScheme::Inverted));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert_eq!(settings.button_scheme, ButtonScheme::Inverted);
            assert!(result.0.is_some());
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = ButtonScheme;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }

    mod settings_retention {
        use super::*;

        #[test]
        fn apply_text_parses_and_updates() {
            let setting = SettingsRetention;
            let mut settings = Settings::default();

            let display = setting.apply_text("10", &mut settings);

            assert_eq!(display, "10");
            assert_eq!(settings.settings_retention, 10);
        }

        #[test]
        fn apply_text_ignores_invalid_input() {
            let setting = SettingsRetention;
            let mut settings = Settings {
                settings_retention: 3,
                ..Default::default()
            };

            let display = setting.apply_text("invalid", &mut settings);

            assert_eq!(settings.settings_retention, 3);
            assert_eq!(display, "3");
        }
    }
}
