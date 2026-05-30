//! Setting kinds for the Reader category.

use super::{SettingData, SettingIdentity, SettingKind, WidgetKind};
use crate::fl;
use crate::geom::Rectangle;
use crate::i18n::I18nDisplay;
use crate::settings::{FileExtension, FinishedAction, RefreshRatePair, Settings};
use crate::view::{Bus, EntryId, EntryKind, Event, ViewId};

/// Reader finished action setting
pub struct FinishedActionSetting;

/// File kinds rendered with dithering.
pub struct DitheredKindsSetting;

impl SettingKind for DitheredKindsSetting {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::DitheredKinds
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-reader-dithered-kinds")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let entries = FileExtension::all()
            .iter()
            .copied()
            .map(|kind| {
                EntryKind::CheckBox(
                    kind.to_string().to_uppercase(),
                    EntryId::ToggleDitheredKind(kind),
                    settings.reader.dithered_kinds.contains(&kind),
                )
            })
            .collect();

        SettingData {
            value: kinds_summary(settings.reader.dithered_kinds.len()),
            widget: WidgetKind::SubMenu(entries),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::ToggleDitheredKind(kind)) = evt {
            if !settings.reader.dithered_kinds.remove(kind) {
                settings.reader.dithered_kinds.insert(*kind);
            }

            return (
                Some(kinds_summary(settings.reader.dithered_kinds.len())),
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

/// Shows global refresh rate and opens the RefreshRateByKindEditor on tap.
pub struct RefreshRateInfo;

impl SettingKind for RefreshRateInfo {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::RefreshRate
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-reader-refresh-rate")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let global = &settings.reader.refresh_rate.global;
        let value = format!("{} / {}", global.regular, global.inverted);

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::OpenRefreshRateEditor),
        }
    }

    /// Updates the summary label when either global input is submitted.
    ///
    /// Settings are intentionally not written here. Each input (`RefreshRateRegularInput`,
    /// `RefreshRateInvertedInput`) has its own [`SettingKind`] row (`RefreshRateRegularSetting`,
    /// `RefreshRateInvertedSetting`) that owns the write.
    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        let global = &settings.reader.refresh_rate.global;
        match evt {
            Event::Submit(ViewId::RefreshRateRegularInput, text) => {
                let regular = text.parse::<u8>().unwrap_or(global.regular);
                (
                    Some(fl!(
                        "settings-reader-refresh-rate-summary",
                        regular = regular,
                        inverted = global.inverted
                    )),
                    false,
                )
            }
            Event::Submit(ViewId::RefreshRateInvertedInput, text) => {
                let inverted = text.parse::<u8>().unwrap_or(global.inverted);
                (
                    Some(fl!(
                        "settings-reader-refresh-rate-summary",
                        regular = global.regular,
                        inverted = inverted
                    )),
                    false,
                )
            }
            _ => (None, false),
        }
    }
}

/// Shows the refresh rate pair for a specific file extension.
pub struct RefreshRateByKindInfo(pub FileExtension);

impl SettingKind for RefreshRateByKindInfo {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::RefreshRateByKind(self.0.as_str().to_string())
    }

    fn label(&self, _settings: &Settings) -> String {
        self.0.to_string().to_uppercase()
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let pair = settings
            .reader
            .refresh_rate
            .by_kind
            .get(self.0.as_str())
            .cloned()
            .unwrap_or(RefreshRatePair {
                regular: 0,
                inverted: 0,
            });

        let value = format!("{} / {}", pair.regular, pair.inverted);

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditRefreshRateByKind(self.0))),
        }
    }

    fn hold_event(&self, rect: Rectangle) -> Option<Event> {
        let entries = vec![EntryKind::Command(
            fl!("delete"),
            EntryId::DeleteRefreshRateByKind(self.0),
        )];

        Some(Event::SubMenu(rect, entries))
    }
}

/// The "regular" field of the global refresh rate pair.
pub struct RefreshRateRegularSetting;

impl SettingKind for RefreshRateRegularSetting {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::RefreshRateRegular
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-reader-refresh-rate-regular")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let value = settings.reader.refresh_rate.global.regular.to_string();

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::OpenNamedInput {
                view_id: crate::view::ViewId::RefreshRateRegularInput,
                label: fl!("settings-reader-refresh-rate-regular-input"),
                max_chars: 3,
                initial_text: settings.reader.refresh_rate.global.regular.to_string(),
            }),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Submit(crate::view::ViewId::RefreshRateRegularInput, text) = evt
            && let Ok(v) = text.parse::<u8>()
        {
            settings.reader.refresh_rate.global.regular = v;
            return (Some(v.to_string()), true);
        }

        (None, false)
    }
}

/// The "inverted" field of the global refresh rate pair.
pub struct RefreshRateInvertedSetting;

impl SettingKind for RefreshRateInvertedSetting {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::RefreshRateInverted
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-reader-refresh-rate-inverted")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let value = settings.reader.refresh_rate.global.inverted.to_string();

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::OpenNamedInput {
                view_id: crate::view::ViewId::RefreshRateInvertedInput,
                label: fl!("settings-reader-refresh-rate-inverted-input"),
                max_chars: 3,
                initial_text: settings.reader.refresh_rate.global.inverted.to_string(),
            }),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Submit(crate::view::ViewId::RefreshRateInvertedInput, text) = evt
            && let Ok(v) = text.parse::<u8>()
        {
            settings.reader.refresh_rate.global.inverted = v;
            return (Some(v.to_string()), true);
        }

        (None, false)
    }
}

/// The "regular" field of a per-kind refresh rate pair.
pub struct RefreshRateByKindRegular(pub FileExtension);

impl SettingKind for RefreshRateByKindRegular {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::RefreshRateByKindRegular(self.0.as_str().to_string())
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-reader-refresh-rate-regular")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let regular = settings
            .reader
            .refresh_rate
            .by_kind
            .get(self.0.as_str())
            .map(|p| p.regular)
            .unwrap_or(0);

        SettingData {
            value: regular.to_string(),
            widget: WidgetKind::ActionLabel(Event::OpenNamedInput {
                view_id: crate::view::ViewId::RefreshRateByKindRegularInput,
                label: fl!(
                    "settings-reader-refresh-rate-by-kind-regular-input",
                    ext = self.0.as_str()
                ),
                max_chars: 3,
                initial_text: regular.to_string(),
            }),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Submit(crate::view::ViewId::RefreshRateByKindRegularInput, text) = evt
            && let Ok(v) = text.parse::<u8>()
        {
            let pair = settings
                .reader
                .refresh_rate
                .by_kind
                .entry(self.0.as_str().to_string())
                .or_insert(RefreshRatePair {
                    regular: 0,
                    inverted: 0,
                });
            pair.regular = v;
            return (Some(v.to_string()), true);
        }

        (None, false)
    }
}

/// The "inverted" field of a per-kind refresh rate pair.
pub struct RefreshRateByKindInverted(pub FileExtension);

impl SettingKind for RefreshRateByKindInverted {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::RefreshRateByKindInverted(self.0.as_str().to_string())
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-reader-refresh-rate-inverted")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let inverted = settings
            .reader
            .refresh_rate
            .by_kind
            .get(self.0.as_str())
            .map(|p| p.inverted)
            .unwrap_or(0);

        SettingData {
            value: inverted.to_string(),
            widget: WidgetKind::ActionLabel(Event::OpenNamedInput {
                view_id: crate::view::ViewId::RefreshRateByKindInvertedInput,
                label: fl!(
                    "settings-reader-refresh-rate-by-kind-inverted-input",
                    ext = self.0.as_str()
                ),
                max_chars: 3,
                initial_text: inverted.to_string(),
            }),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Submit(crate::view::ViewId::RefreshRateByKindInvertedInput, text) = evt
            && let Ok(v) = text.parse::<u8>()
        {
            let pair = settings
                .reader
                .refresh_rate
                .by_kind
                .entry(self.0.as_str().to_string())
                .or_insert(RefreshRatePair {
                    regular: 0,
                    inverted: 0,
                });
            pair.inverted = v;
            return (Some(v.to_string()), true);
        }

        (None, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FileExtension, FinishedAction, Settings};
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

        mod dithered_kinds_setting {
            use super::*;

            #[test]
            fn fetch_builds_checkbox_submenu_for_all_extensions() {
                let setting = DitheredKindsSetting;
                let settings = Settings::default();

                let data = setting.fetch(&settings);

                assert_eq!(
                    data.value,
                    kinds_summary(settings.reader.dithered_kinds.len())
                );
                let WidgetKind::SubMenu(entries) = data.widget else {
                    panic!("expected submenu widget");
                };
                assert_eq!(entries.len(), FileExtension::all().len());
                assert!(matches!(
                    entries.first(),
                    Some(EntryKind::CheckBox(_, EntryId::ToggleDitheredKind(_), _))
                ));
            }

            #[test]
            fn handle_toggle_adds_and_removes_extensions() {
                let setting = DitheredKindsSetting;
                let mut settings = Settings::default();
                settings.reader.dithered_kinds.remove(&FileExtension::Pdf);
                let mut bus: Bus = VecDeque::new();

                let add = setting.handle(
                    &Event::Select(EntryId::ToggleDitheredKind(FileExtension::Pdf)),
                    &mut settings,
                    &mut bus,
                );
                assert_eq!(
                    add.0,
                    Some(kinds_summary(settings.reader.dithered_kinds.len()))
                );
                assert!(settings.reader.dithered_kinds.contains(&FileExtension::Pdf));

                let remove = setting.handle(
                    &Event::Select(EntryId::ToggleDitheredKind(FileExtension::Pdf)),
                    &mut settings,
                    &mut bus,
                );
                assert_eq!(
                    remove.0,
                    Some(kinds_summary(settings.reader.dithered_kinds.len()))
                );
                assert!(!settings.reader.dithered_kinds.contains(&FileExtension::Pdf));
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

    mod refresh_rate_info {
        use super::*;

        #[test]
        fn handle_regular_submit_updates_display_without_writing_settings() {
            let setting = RefreshRateInfo;
            let mut settings = Settings::default();
            settings.reader.refresh_rate.global.regular = 5;
            settings.reader.refresh_rate.global.inverted = 10;
            let mut bus: Bus = VecDeque::new();

            let event = Event::Submit(ViewId::RefreshRateRegularInput, "3".to_string());
            let (display, handled) = setting.handle(&event, &mut settings, &mut bus);

            assert_eq!(
                display.as_deref(),
                Some(
                    fl!(
                        "settings-reader-refresh-rate-summary",
                        regular = 3u8,
                        inverted = 10u8
                    )
                    .as_str()
                )
            );
            assert!(!handled);
            assert_eq!(settings.reader.refresh_rate.global.regular, 5);
        }

        #[test]
        fn handle_inverted_submit_updates_display_without_writing_settings() {
            let setting = RefreshRateInfo;
            let mut settings = Settings::default();
            settings.reader.refresh_rate.global.regular = 5;
            settings.reader.refresh_rate.global.inverted = 10;
            let mut bus: Bus = VecDeque::new();

            let event = Event::Submit(ViewId::RefreshRateInvertedInput, "7".to_string());
            let (display, handled) = setting.handle(&event, &mut settings, &mut bus);

            assert_eq!(
                display.as_deref(),
                Some(
                    fl!(
                        "settings-reader-refresh-rate-summary",
                        regular = 5u8,
                        inverted = 7u8
                    )
                    .as_str()
                )
            );
            assert!(!handled);
            assert_eq!(settings.reader.refresh_rate.global.inverted, 10);
        }

        #[test]
        fn handle_invalid_text_falls_back_to_current_value() {
            let setting = RefreshRateInfo;
            let mut settings = Settings::default();
            settings.reader.refresh_rate.global.regular = 5;
            settings.reader.refresh_rate.global.inverted = 10;
            let mut bus: Bus = VecDeque::new();

            let event = Event::Submit(ViewId::RefreshRateRegularInput, "bad".to_string());
            let (display, _) = setting.handle(&event, &mut settings, &mut bus);

            assert_eq!(
                display.as_deref(),
                Some(
                    fl!(
                        "settings-reader-refresh-rate-summary",
                        regular = 5u8,
                        inverted = 10u8
                    )
                    .as_str()
                )
            );
        }

        #[test]
        fn handle_unrelated_event_returns_none() {
            let setting = RefreshRateInfo;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let (display, handled) =
                setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(display.is_none());
            assert!(!handled);
        }
    }
}
