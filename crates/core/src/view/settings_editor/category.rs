use crate::view::settings_editor::SettingsCategory;

/// The category enum is the one Slint generates. [`SettingsCategory::all`] is
/// the order the hardware buttons step through, matching `setting_line.slint`.
impl SettingsCategory {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::view::settings_editor::components::shown_kinds;
    use crate::view::settings_editor::kinds::SettingIdentity;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn power_omits_soft_suspend_settings_when_unsupported() {
        let context = create_test_context().await;
        let identities: Vec<_> = shown_kinds(SettingsCategory::Power, &context)
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
        let identities: Vec<_> = shown_kinds(SettingsCategory::Power, &context)
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
        let identities: Vec<_> = shown_kinds(SettingsCategory::Reader, &context)
            .iter()
            .map(|setting| setting.identity())
            .collect();

        assert_eq!(identities[0], SettingIdentity::FontFamily);
    }
}
