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
    AutoTime,
    AutoFrontlight,
    AutoFrontlightBrightness,
    AutoFrontlightManualCoordinates,
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
    DbBackupRetention,
    LogLevel,
    ImportSyncMetadata,
    AllowedKinds,
    ForceFullImport,
    #[cfg(feature = "tracing")]
    OtlpEndpoint,
    #[cfg(feature = "profiling")]
    PyroscopeEndpoint,
    #[cfg(all(feature = "test", feature = "kobo"))]
    EnableKernLog,
    #[cfg(all(feature = "test", feature = "kobo"))]
    EnableDbusLog,
    /// Identity for a monolingual dictionary row, keyed by ISO 639-1 language code.
    DictionaryInfo(String),
    /// Summary row in the Reader category showing "regular / inverted".
    RefreshRate,
    /// Global refresh rate (regular, non-inverted page turns).
    RefreshRateRegular,
    /// Global refresh rate (inverted page turns).
    RefreshRateInverted,
    /// Per-kind refresh rate row in the Reader category list.
    RefreshRateByKind(String),
    /// Regular refresh rate inside a per-kind editor.
    RefreshRateByKindRegular(String),
    /// Inverted refresh rate inside a per-kind editor.
    RefreshRateByKindInverted(String),
    DitheredKinds,
    StartupMode,
}
