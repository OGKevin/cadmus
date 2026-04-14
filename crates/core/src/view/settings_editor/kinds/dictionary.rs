//! Setting kinds for the Dictionaries category.

use super::{SettingData, SettingIdentity, SettingKind, WidgetKind};
use crate::fl;
use crate::settings::Settings;
use crate::view::{EntryId, EntryKind, Event};

/// Represents a single monolingual dictionary row in the Dictionaries settings category.
///
/// Each row shows a lang code as the label and "Installed" or "Download" as the
/// value. Installed dictionaries show a sub-menu with "Re-download" and "Delete"
/// options; uninstalled ones show an `ActionLabel` that fires the download event on tap.
/// When an update is available, the value shows "Update Available" and the submenu
/// includes an "Update" option above "Re-download". When a download is in progress,
/// the value shows "Downloading" and no action widget is offered.
pub struct DictionaryInfo {
    /// ISO 639-1 language code, e.g. `"en"` or `"fr"`.
    pub lang: String,
    /// Whether this dictionary is currently installed on the device.
    pub is_installed: bool,
    /// Whether a newer version is available on the server.
    pub update_available: bool,
    /// Whether a download/install is currently in progress for this language.
    pub is_installing: bool,
}

impl SettingKind for DictionaryInfo {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::DictionaryInfo(self.lang.clone())
    }

    fn label(&self, _settings: &Settings) -> String {
        self.lang.clone()
    }

    fn handle(
        &self,
        evt: &Event,
        _settings: &mut Settings,
        _bus: &mut crate::view::Bus,
    ) -> (Option<String>, bool) {
        match evt {
            Event::Select(entry) => match entry {
                EntryId::DownloadDictionary(lang) | EntryId::RedownloadDictionary(lang)
                    if lang == &self.lang =>
                {
                    (Some(fl!("settings-dictionaries-downloading")), false)
                }
                _ => (None, false),
            },
            _ => (None, false),
        }
    }

    fn fetch(&self, _settings: &Settings) -> SettingData {
        if self.is_installing {
            return SettingData {
                value: fl!("settings-dictionaries-downloading"),
                widget: WidgetKind::None,
            };
        }

        if self.is_installed {
            let mut entries = Vec::new();

            if self.update_available {
                entries.push(EntryKind::Command(
                    fl!("settings-dictionaries-update"),
                    EntryId::RedownloadDictionary(self.lang.clone()),
                ));
            } else {
                entries.push(EntryKind::Command(
                    fl!("settings-dictionaries-re-download"),
                    EntryId::RedownloadDictionary(self.lang.clone()),
                ));
            }

            entries.push(EntryKind::Command(
                fl!("settings-dictionaries-delete"),
                EntryId::DeleteDictionary(self.lang.clone()),
            ));

            let value = if self.update_available {
                fl!("settings-dictionaries-update-available")
            } else {
                fl!("settings-dictionaries-installed")
            };

            SettingData {
                value,
                widget: WidgetKind::SubMenu(entries),
            }
        } else {
            SettingData {
                value: fl!("settings-dictionaries-download"),
                widget: WidgetKind::ActionLabel(Event::Select(EntryId::DownloadDictionary(
                    self.lang.clone(),
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::view::Bus;
    use std::collections::VecDeque;

    fn make_settings() -> Settings {
        Settings::default()
    }

    mod fetch {
        use super::*;

        #[test]
        fn uninstalled_yields_action_label_with_download_event() {
            let info = DictionaryInfo {
                lang: "en".to_string(),
                is_installed: false,
                update_available: false,
                is_installing: false,
            };
            let data = info.fetch(&make_settings());

            assert!(matches!(
                data.widget,
                WidgetKind::ActionLabel(Event::Select(EntryId::DownloadDictionary(ref l)))
                    if l == "en"
            ));
        }

        #[test]
        fn installed_yields_submenu_with_redownload_and_delete() {
            let info = DictionaryInfo {
                lang: "fr".to_string(),
                is_installed: true,
                update_available: false,
                is_installing: false,
            };
            let data = info.fetch(&make_settings());

            let WidgetKind::SubMenu(entries) = data.widget else {
                panic!("expected SubMenu");
            };
            assert_eq!(entries.len(), 2);
            assert!(matches!(
                &entries[0],
                EntryKind::Command(_, EntryId::RedownloadDictionary(l)) if l == "fr"
            ));
            assert!(matches!(
                &entries[1],
                EntryKind::Command(_, EntryId::DeleteDictionary(l)) if l == "fr"
            ));
        }

        #[test]
        fn update_available_yields_submenu_with_update_first() {
            let info = DictionaryInfo {
                lang: "de".to_string(),
                is_installed: true,
                update_available: true,
                is_installing: false,
            };
            let data = info.fetch(&make_settings());

            let WidgetKind::SubMenu(entries) = data.widget else {
                panic!("expected SubMenu");
            };
            assert_eq!(entries.len(), 2);
            assert!(matches!(
                &entries[0],
                EntryKind::Command(label, EntryId::RedownloadDictionary(l))
                    if l == "de" && label == "Update"
            ));
            assert!(matches!(
                &entries[1],
                EntryKind::Command(_, EntryId::DeleteDictionary(l)) if l == "de"
            ));
            assert_eq!(data.value, "Update Available");
        }

        #[test]
        fn is_installing_yields_none_widget() {
            let info = DictionaryInfo {
                lang: "es".to_string(),
                is_installed: false,
                update_available: false,
                is_installing: true,
            };
            let data = info.fetch(&make_settings());

            assert!(matches!(data.widget, WidgetKind::None));
        }

        #[test]
        fn is_installing_takes_priority_over_installed() {
            let info = DictionaryInfo {
                lang: "es".to_string(),
                is_installed: true,
                update_available: true,
                is_installing: true,
            };
            let data = info.fetch(&make_settings());

            assert!(matches!(data.widget, WidgetKind::None));
        }
    }

    mod handle {
        use super::*;

        #[test]
        fn download_event_returns_downloading_string() {
            let info = DictionaryInfo {
                lang: "en".to_string(),
                is_installed: false,
                update_available: false,
                is_installing: false,
            };
            let mut settings = make_settings();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::DownloadDictionary("en".to_string()));

            let (display, consumed) = info.handle(&event, &mut settings, &mut bus);

            assert!(display.is_some());
            assert!(!consumed);
        }

        #[test]
        fn redownload_event_returns_downloading_string() {
            let info = DictionaryInfo {
                lang: "en".to_string(),
                is_installed: true,
                update_available: false,
                is_installing: false,
            };
            let mut settings = make_settings();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::RedownloadDictionary("en".to_string()));

            let (display, consumed) = info.handle(&event, &mut settings, &mut bus);

            assert!(display.is_some());
            assert!(!consumed);
        }

        #[test]
        fn event_for_different_lang_returns_none() {
            let info = DictionaryInfo {
                lang: "en".to_string(),
                is_installed: false,
                update_available: false,
                is_installing: false,
            };
            let mut settings = make_settings();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::DownloadDictionary("fr".to_string()));

            let (display, _) = info.handle(&event, &mut settings, &mut bus);

            assert!(display.is_none());
        }

        #[test]
        fn unrelated_event_returns_none() {
            let info = DictionaryInfo {
                lang: "en".to_string(),
                is_installed: false,
                update_available: false,
                is_installing: false,
            };
            let mut settings = make_settings();
            let mut bus: Bus = VecDeque::new();

            let (display, _) = info.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(display.is_none());
        }
    }
}
