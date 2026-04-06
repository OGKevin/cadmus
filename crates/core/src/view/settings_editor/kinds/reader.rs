//! Setting kinds for the Reader category.

use super::{SettingData, SettingIdentity, SettingKind, WidgetKind};
use crate::fl;
use crate::i18n::I18nDisplay;
use crate::settings::{FinishedAction, Settings};
use crate::view::{Bus, EntryId, EntryKind, Event};

/// Reader finished action setting
pub struct FinishedActionSetting;

impl SettingKind for FinishedActionSetting {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::FinishedAction
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-reader-end-of-book-action")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let current = settings.reader.finished;

        let entries = vec![
            EntryKind::RadioButton(
                FinishedAction::Notify.to_i18n_string(),
                EntryId::SetFinishedAction(FinishedAction::Notify),
                current == FinishedAction::Notify,
            ),
            EntryKind::RadioButton(
                FinishedAction::Close.to_i18n_string(),
                EntryId::SetFinishedAction(FinishedAction::Close),
                current == FinishedAction::Close,
            ),
            EntryKind::RadioButton(
                FinishedAction::GoToNext.to_i18n_string(),
                EntryId::SetFinishedAction(FinishedAction::GoToNext),
                current == FinishedAction::GoToNext,
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
        if let Event::Select(EntryId::SetFinishedAction(action)) = evt {
            settings.reader.finished = *action;
            return (Some(action.to_i18n_string()), true);
        }
        (None, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FinishedAction, Settings};
    use crate::view::{Bus, EntryId, Event};
    use std::collections::VecDeque;

    mod finished_action_setting {
        use super::*;

        #[test]
        fn handle_set_action_updates_settings() {
            let setting = FinishedActionSetting;
            let mut settings = Settings::default();
            settings.reader.finished = FinishedAction::Close;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetFinishedAction(FinishedAction::GoToNext));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(settings.reader.finished, FinishedAction::GoToNext);
        }

        #[test]
        fn handle_can_set_all_actions() {
            let setting = FinishedActionSetting;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            for action in [
                FinishedAction::Notify,
                FinishedAction::Close,
                FinishedAction::GoToNext,
            ] {
                let event = Event::Select(EntryId::SetFinishedAction(action));
                setting.handle(&event, &mut settings, &mut bus);
                assert_eq!(settings.reader.finished, action);
            }
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = FinishedActionSetting;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }

        #[test]
        fn handle_returns_none_for_per_library_entry_id() {
            let setting = FinishedActionSetting;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetLibraryFinishedAction(0, FinishedAction::Notify));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }
}
