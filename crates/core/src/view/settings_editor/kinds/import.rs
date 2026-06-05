//! Setting kinds for the Import category.

use super::{SettingData, SettingIdentity, SettingKind, ToggleSettings, WidgetKind};
use crate::fl;
use crate::settings::{FileExtension, Settings};
use crate::view::{Bus, EntryId, EntryKind, Event, ToggleEvent};

/// Force full library re-import action setting.
pub struct ForceFullImport;

impl SettingKind for ForceFullImport {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::ForceFullImport
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-import-force-full-import")
    }

    fn fetch(&self, _settings: &Settings) -> SettingData {
        SettingData {
            value: fl!("settings-general-trigger"),
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::RequestForceImport)),
        }
    }

    fn handle(
        &self,
        _evt: &Event,
        _settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        (None, false)
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

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::ImportSyncMetadata)) = evt {
            settings.import.sync_metadata = !settings.import.sync_metadata;
            return (Some(settings.import.sync_metadata.to_string()), true);
        }
        (None, false)
    }
}

/// Allowed file extensions setting.
pub struct AllowedKindsSetting;

impl SettingKind for AllowedKindsSetting {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AllowedKinds
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-import-allowed-kinds")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let entries = FileExtension::all()
            .iter()
            .copied()
            .map(|kind| {
                EntryKind::CheckBox(
                    kind.to_string().to_uppercase(),
                    EntryId::ToggleAllowedKind(kind),
                    settings.import.allowed_kinds.contains(&kind),
                )
            })
            .collect();

        SettingData {
            value: kinds_summary(settings.import.allowed_kinds.len()),
            widget: WidgetKind::SubMenu(entries),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::ToggleAllowedKind(kind)) = evt {
            if !settings.import.allowed_kinds.remove(kind) {
                settings.import.allowed_kinds.insert(*kind);
            }

            return (
                Some(kinds_summary(settings.import.allowed_kinds.len())),
                true,
            );
        }

        (None, false)
    }

    fn keep_menu_open(&self) -> bool {
        true
    }
}

fn kinds_summary(selected: usize) -> String {
    format!("{selected} / {}", FileExtension::all().len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FileExtension, Settings};
    use crate::view::{Bus, EntryKind, Event};
    use std::collections::VecDeque;

    mod force_full_import {
        use super::*;

        #[test]
        fn identity_returns_force_full_import() {
            let setting = ForceFullImport;
            assert_eq!(setting.identity(), SettingIdentity::ForceFullImport);
        }

        #[test]
        fn fetch_builds_action_label_requesting_force_import() {
            let setting = ForceFullImport;
            let settings = Settings::default();
            let data = setting.fetch(&settings);

            match data.widget {
                WidgetKind::ActionLabel(Event::Select(EntryId::RequestForceImport)) => {}
                other => panic!("expected ActionLabel(RequestForceImport), got {other:?}"),
            }
        }

        #[test]
        fn handle_ignores_events() {
            let setting = ForceFullImport;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
            assert!(!result.1);
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

            assert!(result.0.is_some());
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

            assert!(result.0.is_some());
            assert!(settings.import.sync_metadata);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = ImportSyncMetadata;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            use crate::view::EntryId;

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
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

            assert!(result.0.is_none());
        }
    }

    mod allowed_kinds_setting {
        use super::*;

        #[test]
        fn fetch_builds_checkbox_submenu_for_all_extensions() {
            let setting = AllowedKindsSetting;
            let settings = Settings::default();

            let data = setting.fetch(&settings);

            assert_eq!(
                data.value,
                kinds_summary(settings.import.allowed_kinds.len())
            );
            let WidgetKind::SubMenu(entries) = data.widget else {
                panic!("expected submenu widget");
            };
            assert_eq!(entries.len(), FileExtension::all().len());
            assert!(matches!(
                entries.first(),
                Some(EntryKind::CheckBox(_, EntryId::ToggleAllowedKind(_), _))
            ));
        }

        #[test]
        fn handle_toggle_adds_and_removes_extensions() {
            let setting = AllowedKindsSetting;
            let mut settings = Settings::default();
            settings.import.allowed_kinds.remove(&FileExtension::Cbr);
            let mut bus: Bus = VecDeque::new();

            let add = setting.handle(
                &Event::Select(EntryId::ToggleAllowedKind(FileExtension::Cbr)),
                &mut settings,
                &mut bus,
            );
            assert_eq!(
                add.0,
                Some(kinds_summary(settings.import.allowed_kinds.len()))
            );
            assert!(settings.import.allowed_kinds.contains(&FileExtension::Cbr));

            let remove = setting.handle(
                &Event::Select(EntryId::ToggleAllowedKind(FileExtension::Cbr)),
                &mut settings,
                &mut bus,
            );
            assert_eq!(
                remove.0,
                Some(kinds_summary(settings.import.allowed_kinds.len()))
            );
            assert!(!settings.import.allowed_kinds.contains(&FileExtension::Cbr));
        }
    }
}
