//! Setting kinds for the Power category.

use super::{
    InputSettingKind, SettingData, SettingIdentity, SettingKind, SettingsFetchData, ToggleSettings,
    WidgetKind,
};
use crate::device::AppContext;
use crate::fl;
use crate::settings::Settings;
use crate::view::{Bus, EntryId, Event, ToggleEvent, ViewId};

/// Auto suspend timeout setting
pub struct AutoSuspend;

impl SettingKind for AutoSuspend {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutoSuspend
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-power-auto-suspend")
    }

    fn fetch(&self, data: SettingsFetchData) -> SettingData {
        let value = if data.settings.auto_suspend == 0.0 {
            fl!("settings-general-never")
        } else {
            format!("{:.1}", data.settings.auto_suspend)
        };

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditAutoSuspend)),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        context: &mut AppContext,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Submit(ViewId::AutoSuspendInput, text) = evt {
            let display = self.apply_text(text, &mut context.settings);
            context.reschedule_auto_suspend_alarm();
            return (Some(display), true);
        }

        (None, false)
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
        fl!("settings-power-auto-suspend-input")
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
        fl!("settings-power-auto-power-off")
    }

    fn fetch(&self, data: SettingsFetchData) -> SettingData {
        let value = if data.settings.auto_power_off == 0.0 {
            fl!("settings-general-never")
        } else {
            format!("{:.1}", data.settings.auto_power_off)
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
        fl!("settings-power-auto-power-off-input")
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
        fl!("settings-power-enable-sleep-cover")
    }

    fn fetch(&self, data: SettingsFetchData) -> SettingData {
        SettingData {
            value: data.settings.sleep_cover.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: data.settings.sleep_cover,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::SleepCover)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        context: &mut AppContext,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::SleepCover)) = evt {
            context.settings.sleep_cover = !context.settings.sleep_cover;
            return (Some(context.settings.sleep_cover.to_string()), true);
        }
        (None, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::view::{Bus, EntryId};
    use std::collections::VecDeque;

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

            assert_eq!(display, fl!("settings-general-never"));
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

        #[test]
        fn handle_submit_reschedules_auto_suspend_alarm() {
            use crate::AlarmType;

            let setting = AutoSuspend;
            let mut context = create_test_context();
            context.settings.auto_suspend = 30.0;
            context.reschedule_auto_suspend_alarm();
            let first = context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .time_until_alarm(AlarmType::AutoSuspend)
                .unwrap();
            let mut bus: Bus = VecDeque::new();

            std::thread::sleep(std::time::Duration::from_millis(20));
            let (display, handled) = setting.handle(
                &Event::Submit(ViewId::AutoSuspendInput, "15.0".to_string()),
                &mut context,
                &mut bus,
            );

            assert!(handled);
            assert_eq!(display.as_deref(), Some("15.0"));
            assert_eq!(context.settings.auto_suspend, 15.0);
            let second = context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .time_until_alarm(AlarmType::AutoSuspend)
                .unwrap();
            assert!((second - 15 * 60).abs() < 2);
            assert!(second < first);
        }

        #[test]
        fn handle_submit_zero_cancels_auto_suspend_alarm() {
            use crate::AlarmType;

            let setting = AutoSuspend;
            let mut context = create_test_context();
            context.settings.auto_suspend = 30.0;
            context.reschedule_auto_suspend_alarm();
            let mut bus: Bus = VecDeque::new();

            let (display, handled) = setting.handle(
                &Event::Submit(ViewId::AutoSuspendInput, "0".to_string()),
                &mut context,
                &mut bus,
            );

            assert!(handled);
            assert_eq!(display, Some(fl!("settings-general-never")));
            assert_eq!(context.settings.auto_suspend, 0.0);
            assert!(
                !context
                    .alarm_manager
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .has_alarm(AlarmType::AutoSuspend)
            );
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

            assert_eq!(display, fl!("settings-general-never"));
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
        use crate::context::test_helpers::create_test_context;

        #[test]
        fn handle_toggle_event_toggles_value() {
            let setting = SleepCover;
            let mut context = create_test_context();
            context.settings = Settings {
                sleep_cover: true,
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::SleepCover));

            let result = setting.handle(&event, &mut context, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(result.0.unwrap(), "false");
            assert!(!context.settings.sleep_cover);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = SleepCover;
            let mut context = create_test_context();
            context.settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut context, &mut bus);

            assert!(result.0.is_none());
        }
    }
}
