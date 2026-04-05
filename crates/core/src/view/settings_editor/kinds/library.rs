//! Setting kinds for the Libraries category.

use super::{SettingData, SettingIdentity, SettingKind, WidgetKind};
use crate::fl;
use crate::geom::Rectangle;
use crate::i18n::I18nDisplay;
use crate::settings::{FinishedAction, Settings};
use crate::view::{EntryId, EntryKind, Event};

/// Shows a summary of a library (path) and opens the library editor on tap.
pub struct LibraryInfo(pub usize);

impl SettingKind for LibraryInfo {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::LibraryInfo(self.0)
    }

    fn label(&self, settings: &Settings) -> String {
        settings
            .libraries
            .get(self.0)
            .map(|lib| lib.name.clone())
            .unwrap_or_else(|| fl!("settings-general-unknown"))
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let value = settings
            .libraries
            .get(self.0)
            .map(|lib| lib.path.display().to_string())
            .unwrap_or_else(|| fl!("settings-general-unknown"));

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::EditLibrary(self.0)),
        }
    }

    fn hold_event(&self, rect: Rectangle) -> Option<Event> {
        let entries = vec![EntryKind::Command(
            fl!("delete"),
            EntryId::DeleteLibrary(self.0),
        )];

        Some(Event::SubMenu(rect, entries))
    }
}

/// Library name editing setting
pub struct LibraryName(pub usize);

impl SettingKind for LibraryName {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::LibraryName(self.0)
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-library-name")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let value = settings
            .libraries
            .get(self.0)
            .map(|lib| lib.name.clone())
            .unwrap_or_else(|| fl!("settings-general-unknown"));

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditLibraryName)),
        }
    }
}

/// Library path editing setting
pub struct LibraryPath(pub usize);

impl SettingKind for LibraryPath {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::LibraryPath(self.0)
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-library-path")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let value = settings
            .libraries
            .get(self.0)
            .map(|lib| lib.path.display().to_string())
            .unwrap_or_else(|| fl!("settings-general-unknown"));

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditLibraryPath)),
        }
    }
}

/// Library finished action setting
pub struct LibraryFinishedAction(pub usize);

impl SettingKind for LibraryFinishedAction {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::LibraryFinishedAction(self.0)
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-library-end-of-book-action")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let index = self.0;
        let current = settings.libraries.get(index).and_then(|lib| lib.finished);

        let value = current
            .map(|action| action.to_i18n_string())
            .unwrap_or_else(|| fl!("settings-library-inherit"));

        let entries = vec![
            EntryKind::RadioButton(
                fl!("settings-library-inherit"),
                EntryId::ClearLibraryFinishedAction(index),
                current.is_none(),
            ),
            EntryKind::RadioButton(
                FinishedAction::Notify.to_i18n_string(),
                EntryId::SetLibraryFinishedAction(index, FinishedAction::Notify),
                current == Some(FinishedAction::Notify),
            ),
            EntryKind::RadioButton(
                FinishedAction::Close.to_i18n_string(),
                EntryId::SetLibraryFinishedAction(index, FinishedAction::Close),
                current == Some(FinishedAction::Close),
            ),
            EntryKind::RadioButton(
                FinishedAction::GoToNext.to_i18n_string(),
                EntryId::SetLibraryFinishedAction(index, FinishedAction::GoToNext),
                current == Some(FinishedAction::GoToNext),
            ),
        ];

        SettingData {
            value,
            widget: WidgetKind::SubMenu(entries),
        }
    }
}
