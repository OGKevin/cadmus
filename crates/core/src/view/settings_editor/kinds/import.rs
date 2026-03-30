//! Setting kinds for the Import category.

use super::{SettingData, SettingIdentity, SettingKind, ToggleSettings, WidgetKind};
use crate::fl;
use crate::settings::Settings;
use crate::view::{Bus, Event, ToggleEvent};

/// Import on startup toggle setting
pub struct ImportStartupTrigger;

impl SettingKind for ImportStartupTrigger {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::ImportStartupTrigger
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-import-startup-trigger")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.import.startup_trigger.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.import.startup_trigger,
                tap_event: Event::Toggle(ToggleEvent::Setting(
                    ToggleSettings::ImportStartupTrigger,
                )),
            },
        }
    }

    fn handle(&self, evt: &Event, settings: &mut Settings, _bus: &mut Bus) -> Option<String> {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::ImportStartupTrigger)) = evt {
            settings.import.startup_trigger = !settings.import.startup_trigger;
            return Some(settings.import.startup_trigger.to_string());
        }
        None
    }
}

/// Sync metadata toggle setting
pub struct ImportSyncMetadata;

impl SettingKind for ImportSyncMetadata {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::ImportSyncMetadata
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-import-sync-metadata")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.import.sync_metadata.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.import.sync_metadata,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::ImportSyncMetadata)),
            },
        }
    }

    fn handle(&self, evt: &Event, settings: &mut Settings, _bus: &mut Bus) -> Option<String> {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::ImportSyncMetadata)) = evt {
            settings.import.sync_metadata = !settings.import.sync_metadata;
            return Some(settings.import.sync_metadata.to_string());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::view::settings_editor::kinds::ToggleSettings;
    use crate::view::{Bus, Event, ToggleEvent};
    use std::collections::VecDeque;

    mod import_startup_trigger {
        use super::*;

        #[test]
        fn handle_toggle_disables_when_enabled() {
            let setting = ImportStartupTrigger;
            let mut settings = Settings::default();
            settings.import.startup_trigger = true;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::ImportStartupTrigger));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.is_some());
            assert!(!settings.import.startup_trigger);
        }

        #[test]
        fn handle_toggle_enables_when_disabled() {
            let setting = ImportStartupTrigger;
            let mut settings = Settings::default();
            settings.import.startup_trigger = false;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::ImportStartupTrigger));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.is_some());
            assert!(settings.import.startup_trigger);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = ImportStartupTrigger;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            use crate::view::EntryId;

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.is_none());
        }

        #[test]
        fn handle_returns_none_for_wrong_toggle() {
            let setting = ImportStartupTrigger;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(
                &Event::Toggle(ToggleEvent::Setting(ToggleSettings::SleepCover)),
                &mut settings,
                &mut bus,
            );

            assert!(result.is_none());
        }
    }

    mod import_sync_metadata {
        use super::*;

        #[test]
        fn handle_toggle_disables_when_enabled() {
            let setting = ImportSyncMetadata;
            let mut settings = Settings::default();
            settings.import.sync_metadata = true;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::ImportSyncMetadata));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.is_some());
            assert!(!settings.import.sync_metadata);
        }

        #[test]
        fn handle_toggle_enables_when_disabled() {
            let setting = ImportSyncMetadata;
            let mut settings = Settings::default();
            settings.import.sync_metadata = false;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::ImportSyncMetadata));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.is_some());
            assert!(settings.import.sync_metadata);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = ImportSyncMetadata;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            use crate::view::EntryId;

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.is_none());
        }

        #[test]
        fn handle_returns_none_for_wrong_toggle() {
            let setting = ImportSyncMetadata;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(
                &Event::Toggle(ToggleEvent::Setting(ToggleSettings::SleepCover)),
                &mut settings,
                &mut bus,
            );

            assert!(result.is_none());
        }
    }
}
