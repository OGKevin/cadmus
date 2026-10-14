use super::kinds::SettingKind;
use super::kinds::dictionary;
use super::kinds::general::{
    AutoFrontlight, AutoFrontlightBrightness, AutoFrontlightManualCoordinates, AutoShare, AutoTime,
    ButtonScheme, DbBackupRetentionSetting, KeyboardLayout, Locale, NtpServer, SettingsRetention,
    StartupModeSetting, WifiIdleTimeout,
};
use super::kinds::import::{AllowedKindsSetting, ForceFullImport, ImportSyncMetadata};
use super::kinds::intermission::{
    IntermissionFillColor, IntermissionPowerOff, IntermissionShare, IntermissionSuspend,
};
use super::kinds::library::LibraryInfo;
use super::kinds::power::{
    AutoPowerOff, AutoSuspend, AutosleepGrace, AutosleepModeSetting, IndicateAutosleepLed,
    SleepCover,
};
use super::kinds::reader::{
    DitheredKindsSetting, FinishedActionSetting, FontFamily, RefreshRateInfo,
};
use super::kinds::telemetry::{LogLevel, LoggingEnabled};
use crate::device::AppContext;
use crate::device::soft_suspend::SoftSuspendBackend as _;
use crate::dictionary::MonolingualDictionaryService;

use crate::view::settings_editor::SettingsCategory;

/// The category enum is the one Slint generates. [`SettingsCategory::all`] is
/// the order the hardware buttons step through, matching `setting_line.slint`.
impl SettingsCategory {
    /// Returns the list of setting kinds for this category.
    ///
    /// Each element is a heap-allocated [`SettingKind`] that fully describes
    /// the label, current value, widget type, and tap event for one row.
    ///
    /// `dict_service` is used only by the [`SettingsCategory::Dictionaries`] variant.
    /// All other categories ignore it. Passing `None` for a `Dictionaries`
    /// category produces an empty list and logs a warning.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(context, dict_service)))]
    pub async fn settings(
        &self,
        context: &AppContext,
        dict_service: Option<&MonolingualDictionaryService>,
    ) -> Vec<Box<dyn SettingKind>> {
        match self {
            SettingsCategory::General => vec![
                Box::new(Locale),
                Box::new(AutoShare),
                Box::new(AutoTime),
                Box::new(NtpServer),
                Box::new(AutoFrontlight),
                Box::new(AutoFrontlightBrightness),
                Box::new(AutoFrontlightManualCoordinates),
                Box::new(WifiIdleTimeout),
                Box::new(ButtonScheme),
                Box::new(KeyboardLayout),
                Box::new(SettingsRetention),
                Box::new(DbBackupRetentionSetting),
                Box::new(StartupModeSetting),
            ],
            SettingsCategory::Power => {
                let mut rows: Vec<Box<dyn SettingKind>> = Vec::new();
                if context.inhibitor.is_supported() {
                    rows.push(Box::new(AutosleepModeSetting::new(
                        context.inhibitor.available_modes(),
                    )));
                    rows.push(Box::new(IndicateAutosleepLed));
                    rows.push(Box::new(AutosleepGrace));
                }
                rows.push(Box::new(AutoSuspend));
                rows.push(Box::new(AutoPowerOff));
                rows.push(Box::new(SleepCover));
                rows
            }
            SettingsCategory::Reader => vec![
                Box::new(FontFamily),
                Box::new(FinishedActionSetting),
                Box::new(DitheredKindsSetting),
                Box::new(RefreshRateInfo),
            ],
            SettingsCategory::Libraries => (0..context.settings.libraries.len())
                .map(|i| Box::new(LibraryInfo(i)) as Box<dyn SettingKind>)
                .collect(),
            SettingsCategory::Intermissions => vec![
                Box::new(IntermissionSuspend),
                Box::new(IntermissionPowerOff),
                Box::new(IntermissionShare),
                Box::new(IntermissionFillColor),
            ],
            SettingsCategory::Import => vec![
                Box::new(ForceFullImport),
                Box::new(ImportSyncMetadata),
                Box::new(AllowedKindsSetting),
            ],
            SettingsCategory::Telemetry => {
                let rows: Vec<Box<dyn SettingKind>> =
                    vec![Box::new(LoggingEnabled), Box::new(LogLevel)];

                #[cfg(any(
                    feature = "tracing",
                    feature = "profiling",
                    all(feature = "test", feature = "kobo")
                ))]
                let mut rows = rows;

                #[cfg(feature = "tracing")]
                {
                    use super::kinds::telemetry::OtlpEndpoint;
                    rows.push(Box::new(OtlpEndpoint));
                }

                #[cfg(feature = "profiling")]
                {
                    use super::kinds::telemetry::PyroscopeEndpoint;
                    rows.push(Box::new(PyroscopeEndpoint));
                }

                #[cfg(all(feature = "test", feature = "kobo"))]
                {
                    use super::kinds::telemetry::EnableDbusLog;
                    use super::kinds::telemetry::EnableKernLog;
                    rows.push(Box::new(EnableKernLog));
                    rows.push(Box::new(EnableDbusLog));
                }
                rows
            }
            SettingsCategory::Dictionaries => {
                let Some(service) = dict_service else {
                    tracing::warn!(
                        "No MonolingualDictionaryService provided for Dictionaries category"
                    );
                    return Vec::new();
                };
                dictionary::load_infos(context, service)
                    .await
                    .into_iter()
                    .map(|info| Box::new(info) as Box<dyn SettingKind>)
                    .collect()
            }
        }
    }

    /// Returns all available categories.
    pub fn all() -> Vec<SettingsCategory> {
        vec![
            SettingsCategory::General,
            SettingsCategory::Power,
            SettingsCategory::Reader,
            SettingsCategory::Dictionaries,
            SettingsCategory::Libraries,
            SettingsCategory::Intermissions,
            SettingsCategory::Import,
            SettingsCategory::Telemetry,
        ]
    }

    /// Returns the number of categories.
    pub fn count() -> usize {
        Self::all().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::view::settings_editor::kinds::SettingIdentity;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn power_omits_soft_suspend_settings_when_unsupported() {
        let context = create_test_context().await;
        let identities: Vec<_> = SettingsCategory::Power
            .settings(&context, None)
            .await
            .iter()
            .map(|setting| setting.identity())
            .collect();

        assert_eq!(
            identities,
            [
                SettingIdentity::AutoSuspend,
                SettingIdentity::AutoPowerOff,
                SettingIdentity::SleepCover,
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(target_os = "linux")]
    async fn power_includes_soft_suspend_settings_when_supported() {
        let mut context = create_test_context().await;
        let _linux = crate::context::test_helpers::install_linux_soft_suspend(&mut context);
        let identities: Vec<_> = SettingsCategory::Power
            .settings(&context, None)
            .await
            .iter()
            .map(|setting| setting.identity())
            .collect();

        assert_eq!(
            identities,
            [
                SettingIdentity::AutosleepMode,
                SettingIdentity::IndicateAutosleepLed,
                SettingIdentity::AutosleepGrace,
                SettingIdentity::AutoSuspend,
                SettingIdentity::AutoPowerOff,
                SettingIdentity::SleepCover,
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reader_includes_font_family() {
        let context = create_test_context().await;
        let identities: Vec<_> = SettingsCategory::Reader
            .settings(&context, None)
            .await
            .iter()
            .map(|setting| setting.identity())
            .collect();

        assert_eq!(identities[0], SettingIdentity::FontFamily);
    }
}
