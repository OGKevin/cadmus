//! Setting kinds for the Power category.

use super::{
    InputSettingKind, SettingData, SettingIdentity, SettingKind, SettingsFetchData, ToggleSettings,
    WidgetKind,
};
use crate::device::AppContext;
use crate::device::reschedule_auto_suspend_alarm;
use crate::device::soft_suspend::SoftSuspendBackend as _;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::fl;
use crate::i18n::I18nDisplay;
use crate::settings::Settings;
use crate::view::{Bus, EntryId, EntryKind, Event, ToggleEvent, ViewId};
use std::time::Duration;

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
            reschedule_auto_suspend_alarm(context);
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

/// Soft-suspend autosleep mode (`Off` / `Freeze` / `Mem`).
pub struct AutosleepModeSetting {
    available: Vec<AutosleepMode>,
}

impl AutosleepModeSetting {
    pub(crate) fn new(available: Vec<AutosleepMode>) -> Self {
        Self { available }
    }
}

fn resolve_autosleep_mode(saved: AutosleepMode, available: &[AutosleepMode]) -> AutosleepMode {
    if available.contains(&saved) {
        saved
    } else {
        AutosleepMode::Off
    }
}

impl SettingKind for AutosleepModeSetting {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutosleepMode
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-power-autosleep-mode")
    }

    fn fetch(&self, data: SettingsFetchData) -> SettingData {
        let current = resolve_autosleep_mode(data.settings.autosleep_mode, &self.available);
        let entries = self
            .available
            .iter()
            .copied()
            .map(|mode| {
                EntryKind::RadioButton(
                    mode.to_i18n_string(),
                    EntryId::SetAutosleepMode(mode),
                    current == mode,
                )
            })
            .collect();

        SettingData {
            value: current.to_i18n_string(),
            widget: WidgetKind::SubMenu(entries),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        context: &mut AppContext,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::SetAutosleepMode(mode)) = evt {
            context.settings.autosleep_mode = *mode;
            context.soft_suspend_session.apply_settings(
                context.settings.autosleep_mode,
                context.settings.indicate_autosleep_led,
                Duration::from_secs_f32(context.settings.autosleep_grace.max(0.0)),
            );
            return (Some(mode.to_i18n_string()), true);
        }
        (None, false)
    }
}

/// Use the status LED to show that soft suspend is armed while awake.
pub struct IndicateAutosleepLed;

impl SettingKind for IndicateAutosleepLed {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::IndicateAutosleepLed
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-power-indicate-autosleep-led")
    }

    fn fetch(&self, data: SettingsFetchData) -> SettingData {
        SettingData {
            value: data.settings.indicate_autosleep_led.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: data.settings.indicate_autosleep_led,
                tap_event: Event::Toggle(ToggleEvent::Setting(
                    ToggleSettings::IndicateAutosleepLed,
                )),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        context: &mut AppContext,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::IndicateAutosleepLed)) = evt {
            context.settings.indicate_autosleep_led = !context.settings.indicate_autosleep_led;
            context.soft_suspend_session.apply_settings(
                context.settings.autosleep_mode,
                context.settings.indicate_autosleep_led,
                Duration::from_secs_f32(context.settings.autosleep_grace.max(0.0)),
            );
            return (
                Some(context.settings.indicate_autosleep_led.to_string()),
                true,
            );
        }
        (None, false)
    }
}

/// Delay after the last soft-suspend lease before writing `wake_unlock`.
pub struct AutosleepGrace;

impl SettingKind for AutosleepGrace {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::AutosleepGrace
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-power-autosleep-grace")
    }

    fn fetch(&self, data: SettingsFetchData) -> SettingData {
        SettingData {
            value: format!("{:.1}", data.settings.autosleep_grace),
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditAutosleepGrace)),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        context: &mut AppContext,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Submit(ViewId::AutosleepGraceInput, text) = evt {
            let display = self.apply_text(text, &mut context.settings);
            context.soft_suspend_session.apply_settings(
                context.settings.autosleep_mode,
                context.settings.indicate_autosleep_led,
                Duration::from_secs_f32(context.settings.autosleep_grace.max(0.0)),
            );
            return (Some(display), true);
        }
        (None, false)
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        Some(self)
    }
}

impl InputSettingKind for AutosleepGrace {
    fn submit_view_id(&self) -> ViewId {
        ViewId::AutosleepGraceInput
    }

    fn open_entry_id(&self) -> EntryId {
        EntryId::EditAutosleepGrace
    }

    fn input_label(&self) -> String {
        fl!("settings-power-autosleep-grace-input")
    }

    fn input_max_chars(&self) -> usize {
        10
    }

    fn current_text(&self, settings: &Settings) -> String {
        format!("{:.1}", settings.autosleep_grace)
    }

    fn apply_text(&self, text: &str, settings: &mut Settings) -> String {
        if let Ok(value) = text.parse::<f32>() {
            settings.autosleep_grace = value.max(0.0);
        }
        format!("{:.1}", settings.autosleep_grace)
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
            reschedule_auto_suspend_alarm(&mut context);
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
            reschedule_auto_suspend_alarm(&mut context);
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

    mod autosleep_mode {
        use super::*;
        use crate::context::test_helpers::create_test_context;
        use crate::view::settings_editor::kinds::SettingsFetchData;

        #[test]
        fn identity_and_label() {
            let setting = AutosleepModeSetting::new(vec![AutosleepMode::Off]);
            assert_eq!(setting.identity(), SettingIdentity::AutosleepMode);
            assert_eq!(
                setting.label(&Settings::default()),
                fl!("settings-power-autosleep-mode")
            );
        }

        #[test]
        fn fetch_builds_radio_submenu_for_available_modes() {
            let available = vec![
                AutosleepMode::Off,
                AutosleepMode::Freeze,
                AutosleepMode::Mem,
            ];
            let setting = AutosleepModeSetting::new(available.clone());
            let settings = Settings::default();

            let data = setting.fetch(SettingsFetchData {
                settings: &settings,
                install_dir: None,
            });

            assert_eq!(data.value, AutosleepMode::Off.to_i18n_string());
            let WidgetKind::SubMenu(entries) = data.widget else {
                panic!("expected submenu widget");
            };
            assert_eq!(entries.len(), available.len());
            assert!(matches!(
                entries.first(),
                Some(EntryKind::RadioButton(
                    _,
                    EntryId::SetAutosleepMode(AutosleepMode::Off),
                    true
                ))
            ));
        }

        #[test]
        fn resolve_falls_back_to_off_when_saved_mode_unavailable() {
            assert_eq!(
                resolve_autosleep_mode(AutosleepMode::Mem, &[AutosleepMode::Off]),
                AutosleepMode::Off
            );
            assert_eq!(
                resolve_autosleep_mode(
                    AutosleepMode::Freeze,
                    &[AutosleepMode::Off, AutosleepMode::Freeze]
                ),
                AutosleepMode::Freeze
            );
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn handle_select_updates_settings_and_session() {
            let setting = AutosleepModeSetting::new(vec![
                AutosleepMode::Off,
                AutosleepMode::Freeze,
                AutosleepMode::Mem,
            ]);
            let mut context = create_test_context();
            let _linux = crate::context::test_helpers::install_linux_soft_suspend(&mut context);
            context.settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetAutosleepMode(AutosleepMode::Off));

            let result = setting.handle(&event, &mut context, &mut bus);

            assert_eq!(result.0, Some(AutosleepMode::Off.to_i18n_string()));
            assert!(result.1);
            assert_eq!(context.settings.autosleep_mode, AutosleepMode::Off);
            assert_eq!(context.soft_suspend_session.mode(), AutosleepMode::Off);
            assert!(bus.is_empty());
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn handle_select_persists_mode_even_if_session_sanitizes() {
            use crate::device::linux::soft_suspend::paths::SoftSuspendPaths;
            use crate::device::soft_suspend::SoftSuspend;
            use std::fs;
            use std::sync::Arc;

            let setting = AutosleepModeSetting::new(vec![
                AutosleepMode::Off,
                AutosleepMode::Freeze,
                AutosleepMode::Mem,
            ]);
            let mut context = create_test_context();
            let (_dir, paths) = SoftSuspendPaths::test_fixture();
            fs::write(&paths.state, "freeze\n").expect("state without mem");
            let session = SoftSuspend::with_paths(paths, None);
            context
                .wifi_session
                .set_soft_suspend_session(Arc::clone(&session));
            context.soft_suspend_session = session;
            context.settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetAutosleepMode(AutosleepMode::Mem));

            let result = setting.handle(&event, &mut context, &mut bus);

            assert_eq!(result.0, Some(AutosleepMode::Mem.to_i18n_string()));
            assert!(result.1);
            assert_eq!(context.settings.autosleep_mode, AutosleepMode::Mem);
            assert_eq!(context.soft_suspend_session.mode(), AutosleepMode::Off);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = AutosleepModeSetting::new(vec![
                AutosleepMode::Off,
                AutosleepMode::Freeze,
                AutosleepMode::Mem,
            ]);
            let mut context = create_test_context();
            context.settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut context, &mut bus);

            assert!(result.0.is_none());
            assert!(!result.1);
        }
    }

    mod indicate_autosleep_led {
        use super::*;
        use crate::context::test_helpers::create_test_context;
        use crate::view::settings_editor::kinds::SettingsFetchData;

        #[test]
        fn identity_and_label() {
            let setting = IndicateAutosleepLed;
            assert_eq!(setting.identity(), SettingIdentity::IndicateAutosleepLed);
            assert_eq!(
                setting.label(&Settings::default()),
                fl!("settings-power-indicate-autosleep-led")
            );
        }

        #[test]
        fn fetch_builds_toggle_widget() {
            let setting = IndicateAutosleepLed;
            let settings = Settings {
                indicate_autosleep_led: true,
                ..Default::default()
            };

            let data = setting.fetch(SettingsFetchData {
                settings: &settings,
                install_dir: None,
            });

            assert_eq!(data.value, "true");
            match data.widget {
                WidgetKind::Toggle {
                    enabled,
                    tap_event:
                        Event::Toggle(ToggleEvent::Setting(ToggleSettings::IndicateAutosleepLed)),
                    ..
                } => assert!(enabled),
                other => panic!("expected IndicateAutosleepLed toggle, got {other:?}"),
            }
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn handle_toggle_event_toggles_value_and_session() {
            let setting = IndicateAutosleepLed;
            let mut context = create_test_context();
            let _linux = crate::context::test_helpers::install_linux_soft_suspend(&mut context);
            context.settings = Settings {
                indicate_autosleep_led: true,
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::IndicateAutosleepLed));

            let result = setting.handle(&event, &mut context, &mut bus);

            assert_eq!(result.0.as_deref(), Some("false"));
            assert!(result.1);
            assert!(!context.settings.indicate_autosleep_led);
            assert!(!context.soft_suspend_session.indicate_autosleep_led());
            assert!(bus.is_empty());
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn handle_toggle_enables_when_disabled() {
            let setting = IndicateAutosleepLed;
            let mut context = create_test_context();
            let _linux = crate::context::test_helpers::install_linux_soft_suspend(&mut context);
            context.settings = Settings {
                indicate_autosleep_led: false,
                ..Default::default()
            };
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::IndicateAutosleepLed));

            let result = setting.handle(&event, &mut context, &mut bus);

            assert_eq!(result.0.as_deref(), Some("true"));
            assert!(result.1);
            assert!(context.settings.indicate_autosleep_led);
            assert!(context.soft_suspend_session.indicate_autosleep_led());
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = IndicateAutosleepLed;
            let mut context = create_test_context();
            context.settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut context, &mut bus);

            assert!(result.0.is_none());
            assert!(!result.1);
        }
    }

    mod autosleep_grace {
        use super::*;
        use crate::context::test_helpers::create_test_context;
        use crate::view::settings_editor::kinds::{InputSettingKind, SettingsFetchData};

        #[test]
        fn identity_and_label() {
            let setting = AutosleepGrace;
            assert_eq!(setting.identity(), SettingIdentity::AutosleepGrace);
            assert_eq!(
                setting.label(&Settings::default()),
                fl!("settings-power-autosleep-grace")
            );
        }

        #[test]
        fn fetch_builds_action_label() {
            let setting = AutosleepGrace;
            let settings = Settings {
                autosleep_grace: 7.5,
                ..Default::default()
            };

            let data = setting.fetch(SettingsFetchData {
                settings: &settings,
                install_dir: None,
            });

            assert_eq!(data.value, "7.5");
            assert!(matches!(
                data.widget,
                WidgetKind::ActionLabel(Event::Select(EntryId::EditAutosleepGrace))
            ));
        }

        #[test]
        fn input_kind_metadata() {
            let setting = AutosleepGrace;
            let settings = Settings {
                autosleep_grace: 3.0,
                ..Default::default()
            };

            assert!(setting.as_input_kind().is_some());
            assert_eq!(setting.submit_view_id(), ViewId::AutosleepGraceInput);
            assert_eq!(setting.open_entry_id(), EntryId::EditAutosleepGrace);
            assert_eq!(
                setting.input_label(),
                fl!("settings-power-autosleep-grace-input")
            );
            assert_eq!(setting.input_max_chars(), 10);
            assert_eq!(setting.current_text(&settings), "3.0");
        }

        #[test]
        fn apply_text_parses_and_updates() {
            let setting = AutosleepGrace;
            let mut settings = Settings::default();

            let display = setting.apply_text("10.0", &mut settings);

            assert_eq!(display, "10.0");
            assert_eq!(settings.autosleep_grace, 10.0);
        }

        #[test]
        fn apply_text_clamps_negative_to_zero() {
            let setting = AutosleepGrace;
            let mut settings = Settings::default();

            let display = setting.apply_text("-1", &mut settings);

            assert_eq!(display, "0.0");
            assert_eq!(settings.autosleep_grace, 0.0);
        }

        #[test]
        fn apply_text_ignores_invalid_input() {
            let setting = AutosleepGrace;
            let mut settings = Settings {
                autosleep_grace: 5.0,
                ..Default::default()
            };

            let display = setting.apply_text("invalid", &mut settings);

            assert_eq!(settings.autosleep_grace, 5.0);
            assert_eq!(display, "5.0");
        }

        #[test]
        #[cfg(target_os = "linux")]
        fn handle_submit_applies_grace_to_session() {
            let setting = AutosleepGrace;
            let mut context = create_test_context();
            let _linux = crate::context::test_helpers::install_linux_soft_suspend(&mut context);
            context.settings = Settings::default();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Submit(ViewId::AutosleepGraceInput, "2.5".into());

            let result = setting.handle(&event, &mut context, &mut bus);

            assert_eq!(result.0.as_deref(), Some("2.5"));
            assert!(result.1);
            assert_eq!(context.settings.autosleep_grace, 2.5);
            assert_eq!(
                context.soft_suspend_session.autosleep_grace(),
                Duration::from_secs_f32(2.5)
            );
            assert!(bus.is_empty());
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = AutosleepGrace;
            let mut context = create_test_context();
            context.settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut context, &mut bus);

            assert!(result.0.is_none());
            assert!(!result.1);
        }
    }
}
