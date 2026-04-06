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
pub struct DictionaryInfo {
    /// ISO 639-1 language code, e.g. `"en"` or `"fr"`.
    pub lang: String,
    /// Whether this dictionary is currently installed on the device.
    pub is_installed: bool,
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
        if self.is_installed {
            let entries = vec![
                EntryKind::Command(
                    fl!("settings-dictionaries-re-download"),
                    EntryId::RedownloadDictionary(self.lang.clone()),
                ),
                EntryKind::Command(
                    fl!("settings-dictionaries-delete"),
                    EntryId::DeleteDictionary(self.lang.clone()),
                ),
            ];

            SettingData {
                value: fl!("settings-dictionaries-installed"),
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
