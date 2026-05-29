use super::kinds::dictionary::DictionaryInfo;
use super::kinds::general::{
    AutoPowerOff, AutoShare, AutoSuspend, ButtonScheme, KeyboardLayout, Locale, SettingsRetention,
    SleepCover,
};
use super::kinds::import::{AllowedKindsSetting, ImportStartupTrigger, ImportSyncMetadata};
use super::kinds::intermission::{IntermissionPowerOff, IntermissionShare, IntermissionSuspend};
use super::kinds::library::LibraryInfo;
use super::kinds::reader::{DitheredKindsSetting, FinishedActionSetting, RefreshRateInfo};
use super::kinds::telemetry::{LogLevel, LoggingEnabled};
use super::kinds::SettingKind;
use crate::context::Context;
use crate::dictionary::MonolingualDictionaryService;
use std::collections::BTreeSet;

/// Categories of settings available in the settings editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// General device settings (auto-suspend, keyboard layout, etc.)
    General,
    /// Reader behavior settings (finished action, etc.)
    Reader,
    /// Library management settings
    Libraries,
    /// Intermission screen display settings
    Intermissions,
    /// Import behavior settings
    Import,
    /// Telemetry and logging settings
    Telemetry,
    /// Monolingual dictionary download and management
    Dictionaries,
}

impl Category {
    /// Returns the display label for this category.
    pub fn label(&self) -> String {
        match self {
            Category::General => "General".to_string(),
            Category::Reader => "Reader".to_string(),
            Category::Libraries => "Libraries".to_string(),
            Category::Intermissions => "Intermission Screens".to_string(),
            Category::Import => "Import".to_string(),
            Category::Telemetry => "Telemetry".to_string(),
            Category::Dictionaries => "Dictionaries".to_string(),
        }
    }

    /// Returns the list of setting kinds for this category.
    ///
    /// Each element is a heap-allocated [`SettingKind`] that fully describes
    /// the label, current value, widget type, and tap event for one row.
    ///
    /// `dict_service` is used only by the [`Category::Dictionaries`] variant.
    /// All other categories ignore it. Passing `None` for a `Dictionaries`
    /// category produces an empty list and logs a warning.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(context, dict_service)))]
    pub fn settings(
        &self,
        context: &Context,
        dict_service: Option<&MonolingualDictionaryService>,
    ) -> Vec<Box<dyn SettingKind>> {
        match self {
            Category::General => vec![
                Box::new(Locale),
                Box::new(AutoShare),
                Box::new(AutoSuspend),
                Box::new(AutoPowerOff),
                Box::new(ButtonScheme),
                Box::new(KeyboardLayout),
                Box::new(SleepCover),
                Box::new(SettingsRetention),
            ],
            Category::Reader => vec![
                Box::new(FinishedActionSetting),
                Box::new(DitheredKindsSetting),
                Box::new(RefreshRateInfo),
            ],
            Category::Libraries => (0..context.settings.libraries.len())
                .map(|i| Box::new(LibraryInfo(i)) as Box<dyn SettingKind>)
                .collect(),
            Category::Intermissions => vec![
                Box::new(IntermissionSuspend),
                Box::new(IntermissionPowerOff),
                Box::new(IntermissionShare),
            ],
            Category::Import => vec![
                Box::new(ImportStartupTrigger),
                Box::new(ImportSyncMetadata),
                Box::new(AllowedKindsSetting),
            ],
            Category::Telemetry => {
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
            Category::Dictionaries => {
                let Some(service) = dict_service else {
                    tracing::warn!(
                        "No MonolingualDictionaryService provided for Dictionaries category"
                    );
                    return Vec::new();
                };

                let available: BTreeSet<String> = if context.online {
                    match service.get_available_dictionaries() {
                        Ok(dicts) => dicts.into_iter().map(|(lang, _)| lang).collect(),
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to load available dictionaries");
                            BTreeSet::new()
                        }
                    }
                } else {
                    BTreeSet::new()
                };

                let installed: BTreeSet<String> = match service.get_installed_dictionaries() {
                    Ok(dicts) => dicts.into_iter().collect(),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to load installed dictionaries");
                        BTreeSet::new()
                    }
                };

                let mut all_langs: Vec<String> = available.union(&installed).cloned().collect();
                all_langs.sort();

                all_langs
                    .into_iter()
                    .map(|lang| {
                        let is_installed = installed.contains(&lang);
                        let update_available = is_installed && service.is_update_available(&lang);
                        let is_installing = service.is_installing(&lang);
                        Box::new(DictionaryInfo {
                            lang,
                            is_installed,
                            update_available,
                            is_installing,
                        }) as Box<dyn SettingKind>
                    })
                    .collect()
            }
        }
    }

    /// Returns all available categories.
    pub fn all() -> Vec<Category> {
        vec![
            Category::General,
            Category::Reader,
            Category::Dictionaries,
            Category::Libraries,
            Category::Intermissions,
            Category::Import,
            Category::Telemetry,
        ]
    }

    /// Returns the number of categories.
    pub fn count() -> usize {
        Self::all().len()
    }
}
