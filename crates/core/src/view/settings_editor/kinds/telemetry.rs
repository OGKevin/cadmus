//! Setting kinds for the Telemetry category.

use super::{SettingData, SettingIdentity, SettingKind, ToggleSettings, WidgetKind};
use crate::fl;
use crate::settings::Settings;
use crate::view::{Bus, EntryId, EntryKind, Event, ToggleEvent};

#[cfg(feature = "otel")]
use super::InputSettingKind;
#[cfg(feature = "otel")]
use crate::view::ViewId;
use std::str::FromStr;

/// Logging enabled toggle setting
pub struct LoggingEnabled;

impl SettingKind for LoggingEnabled {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::LoggingEnabled
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-telemetry-enable-logging")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.logging.enabled.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.logging.enabled,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::LoggingEnabled)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::LoggingEnabled)) = evt {
            settings.logging.enabled = !settings.logging.enabled;
            return (Some(settings.logging.enabled.to_string()), true);
        }
        (None, false)
    }
}

/// Log level selection setting
pub struct LogLevel;

impl LogLevel {
    fn level_to_i18n(level: &tracing::Level) -> String {
        match *level {
            tracing::Level::TRACE => fl!("settings-log-level-trace"),
            tracing::Level::DEBUG => fl!("settings-log-level-debug"),
            tracing::Level::INFO => fl!("settings-log-level-info"),
            tracing::Level::WARN => fl!("settings-log-level-warn"),
            tracing::Level::ERROR => fl!("settings-log-level-error"),
        }
    }
}

impl SettingKind for LogLevel {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::LogLevel
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-telemetry-log-level")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let current = tracing::Level::from_str(settings.logging.level.as_str())
            .unwrap_or(tracing::Level::INFO);

        let entries = vec![
            EntryKind::RadioButton(
                Self::level_to_i18n(&tracing::Level::TRACE),
                EntryId::SetLogLevel(tracing::Level::TRACE),
                current == tracing::Level::TRACE,
            ),
            EntryKind::RadioButton(
                Self::level_to_i18n(&tracing::Level::DEBUG),
                EntryId::SetLogLevel(tracing::Level::DEBUG),
                current == tracing::Level::DEBUG,
            ),
            EntryKind::RadioButton(
                Self::level_to_i18n(&tracing::Level::INFO),
                EntryId::SetLogLevel(tracing::Level::INFO),
                current == tracing::Level::INFO,
            ),
            EntryKind::RadioButton(
                Self::level_to_i18n(&tracing::Level::WARN),
                EntryId::SetLogLevel(tracing::Level::WARN),
                current == tracing::Level::WARN,
            ),
            EntryKind::RadioButton(
                Self::level_to_i18n(&tracing::Level::ERROR),
                EntryId::SetLogLevel(tracing::Level::ERROR),
                current == tracing::Level::ERROR,
            ),
        ];

        SettingData {
            value: Self::level_to_i18n(&current),
            widget: WidgetKind::SubMenu(entries),
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Select(EntryId::SetLogLevel(ref level)) = evt {
            settings.logging.level = level.to_string();
            return (Some(Self::level_to_i18n(level)), true);
        }
        (None, false)
    }
}

/// OTLP endpoint configuration setting (otel feature)
#[cfg(feature = "otel")]
pub struct OtlpEndpoint;

#[cfg(feature = "otel")]
impl SettingKind for OtlpEndpoint {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::OtlpEndpoint
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-telemetry-otlp-endpoint")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        let value = settings
            .logging
            .otlp_endpoint
            .clone()
            .unwrap_or_else(|| fl!("settings-general-not-set"));

        SettingData {
            value,
            widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditOtlpEndpoint)),
        }
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        Some(self)
    }
}

#[cfg(feature = "otel")]
impl InputSettingKind for OtlpEndpoint {
    fn submit_view_id(&self) -> ViewId {
        ViewId::OtlpEndpointInput
    }

    fn open_entry_id(&self) -> EntryId {
        EntryId::EditOtlpEndpoint
    }

    fn input_label(&self) -> String {
        fl!("settings-telemetry-otlp-endpoint")
    }

    fn input_max_chars(&self) -> usize {
        50
    }

    fn current_text(&self, settings: &Settings) -> String {
        settings.logging.otlp_endpoint.clone().unwrap_or_default()
    }

    fn apply_text(&self, text: &str, settings: &mut Settings) -> String {
        let trimmed = text.trim();
        settings.logging.otlp_endpoint = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        settings
            .logging
            .otlp_endpoint
            .clone()
            .unwrap_or_else(|| fl!("settings-general-not-set"))
    }
}

/// Kernel logging toggle setting (test + kobo features)
#[cfg(all(feature = "test", feature = "kobo"))]
pub struct EnableKernLog;

#[cfg(all(feature = "test", feature = "kobo"))]
impl SettingKind for EnableKernLog {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::EnableKernLog
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-telemetry-enable-kernel-log")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.logging.enable_kern_log.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.logging.enable_kern_log,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::EnableKernLog)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::EnableKernLog)) = evt {
            settings.logging.enable_kern_log = !settings.logging.enable_kern_log;
            return (Some(settings.logging.enable_kern_log.to_string()), true);
        }
        (None, false)
    }
}

/// D-Bus logging toggle setting (test + kobo features)
#[cfg(all(feature = "test", feature = "kobo"))]
pub struct EnableDbusLog;

#[cfg(all(feature = "test", feature = "kobo"))]
impl SettingKind for EnableDbusLog {
    fn identity(&self) -> SettingIdentity {
        SettingIdentity::EnableDbusLog
    }

    fn label(&self, _settings: &Settings) -> String {
        fl!("settings-telemetry-enable-dbus-log")
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        SettingData {
            value: settings.logging.enable_dbus_log.to_string(),
            widget: WidgetKind::Toggle {
                left_label: fl!("settings-general-toggle-on"),
                right_label: fl!("settings-general-toggle-off"),
                enabled: settings.logging.enable_dbus_log,
                tap_event: Event::Toggle(ToggleEvent::Setting(ToggleSettings::EnableDbusLog)),
            },
        }
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        if let Event::Toggle(ToggleEvent::Setting(ToggleSettings::EnableDbusLog)) = evt {
            settings.logging.enable_dbus_log = !settings.logging.enable_dbus_log;
            return (Some(settings.logging.enable_dbus_log.to_string()), true);
        }
        (None, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::view::settings_editor::kinds::ToggleSettings;
    use crate::view::{Bus, EntryId, Event, ToggleEvent};
    use std::collections::VecDeque;

    mod logging_enabled {
        use super::*;

        #[test]
        fn handle_toggle_disables_when_enabled() {
            let setting = LoggingEnabled;
            let mut settings = Settings::default();
            settings.logging.enabled = true;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::LoggingEnabled));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert!(!settings.logging.enabled);
        }

        #[test]
        fn handle_toggle_enables_when_disabled() {
            let setting = LoggingEnabled;
            let mut settings = Settings::default();
            settings.logging.enabled = false;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::LoggingEnabled));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert!(settings.logging.enabled);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = LoggingEnabled;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }

        #[test]
        fn handle_returns_none_for_wrong_toggle() {
            let setting = LoggingEnabled;
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

    mod log_level {
        use super::*;

        #[test]
        fn handle_set_level_updates_settings() {
            let setting = LogLevel;
            let mut settings = Settings::default();
            settings.logging.level = "INFO".to_string();
            let mut bus: Bus = VecDeque::new();
            let event = Event::Select(EntryId::SetLogLevel(tracing::Level::WARN));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert_eq!(settings.logging.level, "WARN");
        }

        #[test]
        fn handle_can_set_all_levels() {
            let setting = LogLevel;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            for level in [
                tracing::Level::TRACE,
                tracing::Level::DEBUG,
                tracing::Level::INFO,
                tracing::Level::WARN,
                tracing::Level::ERROR,
            ] {
                let event = Event::Select(EntryId::SetLogLevel(level));
                setting.handle(&event, &mut settings, &mut bus);
                assert_eq!(settings.logging.level, level.to_string());
            }
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = LogLevel;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }
    }

    #[cfg(feature = "otel")]
    mod otlp_endpoint {
        use super::*;
        use crate::view::settings_editor::kinds::InputSettingKind;

        #[test]
        fn apply_text_sets_endpoint() {
            let setting = OtlpEndpoint;
            let mut settings = Settings::default();

            let display = setting.apply_text("http://otel:4317", &mut settings);

            assert_eq!(display, "http://otel:4317");
            assert_eq!(
                settings.logging.otlp_endpoint,
                Some("http://otel:4317".to_string())
            );
        }

        #[test]
        fn apply_text_trims_whitespace() {
            let setting = OtlpEndpoint;
            let mut settings = Settings::default();

            let display = setting.apply_text("  http://otel:4317  ", &mut settings);

            assert_eq!(display, "http://otel:4317");
            assert_eq!(
                settings.logging.otlp_endpoint,
                Some("http://otel:4317".to_string())
            );
        }

        #[test]
        fn apply_text_empty_clears_endpoint() {
            let setting = OtlpEndpoint;
            let mut settings = Settings::default();
            settings.logging.otlp_endpoint = Some("http://old:4317".to_string());

            let display = setting.apply_text("", &mut settings);

            assert_eq!(display, "Not set");
            assert_eq!(settings.logging.otlp_endpoint, None);
        }

        #[test]
        fn apply_text_whitespace_only_clears_endpoint() {
            let setting = OtlpEndpoint;
            let mut settings = Settings::default();
            settings.logging.otlp_endpoint = Some("http://old:4317".to_string());

            let display = setting.apply_text("   ", &mut settings);

            assert_eq!(display, "Not set");
            assert_eq!(settings.logging.otlp_endpoint, None);
        }
    }

    #[cfg(feature = "test")]
    mod enable_kern_log {
        use super::*;

        #[test]
        fn handle_toggle_enables_when_disabled() {
            let setting = EnableKernLog;
            let mut settings = Settings::default();
            settings.logging.enable_kern_log = false;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::EnableKernLog));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert!(settings.logging.enable_kern_log);
        }

        #[test]
        fn handle_toggle_disables_when_enabled() {
            let setting = EnableKernLog;
            let mut settings = Settings::default();
            settings.logging.enable_kern_log = true;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::EnableKernLog));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert!(!settings.logging.enable_kern_log);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = EnableKernLog;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }

        #[test]
        fn handle_returns_none_for_wrong_toggle() {
            let setting = EnableKernLog;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(
                &Event::Toggle(ToggleEvent::Setting(ToggleSettings::LoggingEnabled)),
                &mut settings,
                &mut bus,
            );

            assert!(result.0.is_none());
        }
    }

    #[cfg(all(feature = "test", feature = "kobo"))]
    mod enable_dbus_log {
        use super::*;

        #[test]
        fn handle_toggle_enables_when_disabled() {
            let setting = EnableDbusLog;
            let mut settings = Settings::default();
            settings.logging.enable_dbus_log = false;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::EnableDbusLog));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert!(settings.logging.enable_dbus_log);
        }

        #[test]
        fn handle_toggle_disables_when_enabled() {
            let setting = EnableDbusLog;
            let mut settings = Settings::default();
            settings.logging.enable_dbus_log = true;
            let mut bus: Bus = VecDeque::new();
            let event = Event::Toggle(ToggleEvent::Setting(ToggleSettings::EnableDbusLog));

            let result = setting.handle(&event, &mut settings, &mut bus);

            assert!(result.0.is_some());
            assert!(!settings.logging.enable_dbus_log);
        }

        #[test]
        fn handle_returns_none_for_wrong_event() {
            let setting = EnableDbusLog;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(&Event::Select(EntryId::About), &mut settings, &mut bus);

            assert!(result.0.is_none());
        }

        #[test]
        fn handle_returns_none_for_wrong_toggle() {
            let setting = EnableDbusLog;
            let mut settings = Settings::default();
            let mut bus: Bus = VecDeque::new();

            let result = setting.handle(
                &Event::Toggle(ToggleEvent::Setting(ToggleSettings::LoggingEnabled)),
                &mut settings,
                &mut bus,
            );

            assert!(result.0.is_none());
        }
    }
}
