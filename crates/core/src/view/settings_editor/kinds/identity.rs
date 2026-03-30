//! Single, deduplicated identity enum for all settings.
//!
//! This replaces the two parallel `Kind` enums that previously existed in
//! `setting_row` and `setting_value`.  It is only used to route
//! [`SettingsEvent::UpdateValue`](crate::view::settings_editor::SettingsEvent)
//! events to the correct view — the view layer itself contains no per-setting
//! match arms.

/// Identifies a specific setting value view for targeted updates.
///
/// Used in [`SettingsEvent::UpdateValue`](crate::view::settings_editor::SettingsEvent)
/// so that [`CategoryEditor`](crate::view::settings_editor::CategoryEditor) can tell
/// exactly which [`SettingValue`](crate::view::settings_editor::SettingValue) to
/// refresh after a setting changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingIdentity {
    KeyboardLayout,
    Locale,
    AutoSuspend,
    AutoPowerOff,
    SleepCover,
    AutoShare,
    ButtonScheme,
    LoggingEnabled,
    FinishedAction,
    LibraryInfo(usize),
    LibraryName(usize),
    LibraryPath(usize),
    LibraryFinishedAction(usize),
    IntermissionSuspend,
    IntermissionPowerOff,
    IntermissionShare,
    SettingsRetention,
    LogLevel,
    ImportStartupTrigger,
    ImportSyncMetadata,
    #[cfg(feature = "otel")]
    OtlpEndpoint,
    #[cfg(all(feature = "test", feature = "kobo"))]
    EnableKernLog,
    #[cfg(all(feature = "test", feature = "kobo"))]
    EnableDbusLog,
}
