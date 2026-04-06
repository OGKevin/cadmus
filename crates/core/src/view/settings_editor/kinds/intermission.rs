//! Setting kinds for the Intermissions category.

use super::{SettingData, SettingIdentity, SettingKind, WidgetKind};
use crate::fl;
use crate::i18n::I18nDisplay;
use crate::settings::{IntermKind, IntermissionDisplay, Settings};
use crate::view::{Bus, EntryId, EntryKind, Event};

/// Fetches the intermission display setting data for the given intermission kind
fn fetch_intermission(kind: IntermKind, settings: &Settings) -> SettingData {
    let display = &settings.intermissions[kind];

    let (value, is_logo, is_cover) = match display {
        IntermissionDisplay::Logo => (display.to_i18n_string(), true, false),
        IntermissionDisplay::Cover => (display.to_i18n_string(), false, true),
        IntermissionDisplay::Image(path) => {
            let i18n_display = fl!("settings-intermission-custom");
            let display_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(i18n_display.as_str())
                .to_string();
            (display_name, false, false)
        }
    };

    let entries = vec![
        EntryKind::RadioButton(
            IntermissionDisplay::Logo.to_i18n_string(),
            EntryId::SetIntermission(kind, IntermissionDisplay::Logo),
            is_logo,
        ),
        EntryKind::RadioButton(
            IntermissionDisplay::Cover.to_i18n_string(),
            EntryId::SetIntermission(kind, IntermissionDisplay::Cover),
            is_cover,
        ),
        EntryKind::Command(
            fl!("settings-intermission-custom-image"),
            EntryId::EditIntermissionImage(kind),
        ),
    ];

    SettingData {
        value,
        widget: WidgetKind::SubMenu(entries),
    }
}

/// Extracts the display name from an [`IntermissionDisplay`] value.
///
/// Uses [`IntermissionDisplay`]'s [`I18nDisplay`] implementation for
/// `Logo` and `Cover`. For `Image`, the filename is used instead of the full path since
/// the built-in `Display` impl only yields `"Custom"` for that variant.
fn intermission_display_name(display: &IntermissionDisplay) -> String {
    let i18n_display = fl!("settings-intermission-custom");
    match display {
        IntermissionDisplay::Image(path) => path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(i18n_display.as_str())
            .to_string(),
        _ => display.to_i18n_string(),
    }
}

/// Suspend screen display setting
pub struct IntermissionSuspend;

impl SettingKind for IntermissionSuspend {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::IntermissionSuspend
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-intermission-suspend-screen")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        fetch_intermission(IntermKind::Suspend, settings)
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::SetIntermission(IntermKind::Suspend, ref display)) = evt {
            settings.intermissions[IntermKind::Suspend] = display.clone();
            return (Some(intermission_display_name(display)), true);
        }

        if let Event::FileChooserClosed(Some(ref path)) = evt {
            let display = IntermissionDisplay::Image(path.clone());
            settings.intermissions[IntermKind::Suspend] = display.clone();
            return (Some(intermission_display_name(&display)), true);
        }

        (None, false)
    }

    fn file_chooser_entry_id(&self) -> Option<EntryId> {
        Some(EntryId::EditIntermissionImage(IntermKind::Suspend))
    }
}

/// Power off screen display setting
pub struct IntermissionPowerOff;

impl SettingKind for IntermissionPowerOff {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::IntermissionPowerOff
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-intermission-power-off-screen")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        fetch_intermission(IntermKind::PowerOff, settings)
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::SetIntermission(IntermKind::PowerOff, ref display)) = evt {
            settings.intermissions[IntermKind::PowerOff] = display.clone();
            return (Some(intermission_display_name(display)), true);
        }

        if let Event::FileChooserClosed(Some(ref path)) = evt {
            let display = IntermissionDisplay::Image(path.clone());
            settings.intermissions[IntermKind::PowerOff] = display.clone();
            return (Some(intermission_display_name(&display)), true);
        }

        (None, false)
    }

    fn file_chooser_entry_id(&self) -> Option<EntryId> {
        Some(EntryId::EditIntermissionImage(IntermKind::PowerOff))
    }
}

/// Share screen display setting
pub struct IntermissionShare;

impl SettingKind for IntermissionShare {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::IntermissionShare
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-intermission-share-screen")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        fetch_intermission(IntermKind::Share, settings)
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::SetIntermission(IntermKind::Share, ref display)) = evt {
            settings.intermissions[IntermKind::Share] = display.clone();
            return (Some(intermission_display_name(display)), true);
        }

        if let Event::FileChooserClosed(Some(ref path)) = evt {
            let display = IntermissionDisplay::Image(path.clone());
            settings.intermissions[IntermKind::Share] = display.clone();
            return (Some(intermission_display_name(&display)), true);
        }

        (None, false)
    }

    fn file_chooser_entry_id(&self) -> Option<EntryId> {
        Some(EntryId::EditIntermissionImage(IntermKind::Share))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{IntermissionDisplay, Settings};
    use crate::view::{Bus, EntryId, Event};
    use std::collections::VecDeque;
    use std::path::PathBuf;

    mod intermission_suspend {
        use super::*;

        #[test]
        fn handle_set_intermission_updates_settings() {
            let setting = IntermissionSuspend;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetIntermission(
                IntermKind::Suspend,
                IntermissionDisplay::Cover,
            ));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(
                settings.intermissions[IntermKind::Suspend],
                IntermissionDisplay::Cover
            );
        }

        #[test]
        fn handle_file_chooser_closed_updates_settings() {
            let setting = IntermissionSuspend;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let path = PathBuf::from("/selected/image.jpg");
            let event = Event::FileChooserClosed(Some(path));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(
                settings.intermissions[IntermKind::Suspend],
                IntermissionDisplay::Image(PathBuf::from("/selected/image.jpg"))
            );
        }

        #[test]
        fn handle_returns_none_for_wrong_kind() {
            let setting = IntermissionSuspend;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetIntermission(
                IntermKind::PowerOff,
                IntermissionDisplay::Cover,
            ));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_none());
        }

        #[test]
        fn handle_returns_none_for_cancelled_file_chooser() {
            let setting = IntermissionSuspend;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::FileChooserClosed(None), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }

    mod intermission_power_off {
        use super::*;

        #[test]
        fn handle_set_intermission_updates_settings() {
            let setting = IntermissionPowerOff;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetIntermission(
                IntermKind::PowerOff,
                IntermissionDisplay::Cover,
            ));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(
                settings.intermissions[IntermKind::PowerOff],
                IntermissionDisplay::Cover
            );
        }

        #[test]
        fn handle_file_chooser_closed_updates_settings() {
            let setting = IntermissionPowerOff;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let path = PathBuf::from("/selected/poweroff.png");
            let event = Event::FileChooserClosed(Some(path));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(
                settings.intermissions[IntermKind::PowerOff],
                IntermissionDisplay::Image(PathBuf::from("/selected/poweroff.png"))
            );
        }

        #[test]
        fn handle_returns_none_for_wrong_kind() {
            let setting = IntermissionPowerOff;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetIntermission(
                IntermKind::Suspend,
                IntermissionDisplay::Logo,
            ));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_none());
        }

        #[test]
        fn handle_returns_none_for_cancelled_file_chooser() {
            let setting = IntermissionPowerOff;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::FileChooserClosed(None), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }

    mod intermission_share {
        use super::*;

        #[test]
        fn handle_set_intermission_updates_settings() {
            let setting = IntermissionShare;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetIntermission(
                IntermKind::Share,
                IntermissionDisplay::Cover,
            ));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(
                settings.intermissions[IntermKind::Share],
                IntermissionDisplay::Cover
            );
        }

        #[test]
        fn handle_file_chooser_closed_updates_settings() {
            let setting = IntermissionShare;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let path = PathBuf::from("/selected/share.jpg");
            let event = Event::FileChooserClosed(Some(path));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(
                settings.intermissions[IntermKind::Share],
                IntermissionDisplay::Image(PathBuf::from("/selected/share.jpg"))
            );
        }

        #[test]
        fn handle_returns_none_for_wrong_kind() {
            let setting = IntermissionShare;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetIntermission(
                IntermKind::PowerOff,
                IntermissionDisplay::Cover,
            ));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_none());
        }

        #[test]
        fn handle_returns_none_for_cancelled_file_chooser() {
            let setting = IntermissionShare;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::FileChooserClosed(None), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }
}
