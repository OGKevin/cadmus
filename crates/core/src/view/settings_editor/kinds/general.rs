//! Setting kinds for the General category.

use super::{
    InputSettingKind, SettingData, SettingIdentity, SettingKind, ToggleSettings, WidgetKind,
};
use crate::fl;
use crate::i18n::I18nDisplay;
use crate::settings::Settings;
use crate::view::{Bus, EntryId, EntryKind, Event, ToggleEvent, ViewId};
use anyhow::Error;
use std::fs;
use std::path::Path;

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
        if let Event::Select(EntryId::SetLocale(ref locale)) = evt {
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
        if let Event::Select(EntryId::SetKeyboardLayout(ref layout)) = evt {
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

/// Scans the keyboard-layouts directory for available keyboard layouts
fn get_available_layouts() -> Result<Vec<String>, Error> {
    let layouts_dir = Path::new("keyboard-layouts");
    let mut layouts = Vec::new();

    if layouts_dir.exists() {
        for entry in fs::read_dir(layouts_dir)? {
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
