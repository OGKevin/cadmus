use super::kinds::general::{
    AutoPowerOff, AutoShare, AutoSuspend, ButtonScheme, KeyboardLayout, Locale, SettingsRetention,
    SleepCover,
};
use super::kinds::import::{ImportStartupTrigger, ImportSyncMetadata};
use super::kinds::intermission::{IntermissionPowerOff, IntermissionShare, IntermissionSuspend};
use super::kinds::library::LibraryInfo;
use super::kinds::reader::FinishedActionSetting;
use super::kinds::telemetry::{LogLevel, LoggingEnabled};
use super::kinds::SettingKind;
use crate::context::Context;

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
        }
    }

    /// Returns the list of setting kinds for this category.
    ///
    /// Each element is a heap-allocated [`SettingKind`] that fully describes
    /// the label, current value, widget type, and tap event for one row.
    pub fn settings(&self, context: &Context) -> Vec<Box<dyn SettingKind>> {
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
            Category::Reader => vec![Box::new(FinishedActionSetting)],
            Category::Libraries => (0..context.settings.libraries.len())
                .map(|i| Box::new(LibraryInfo(i)) as Box<dyn SettingKind>)
                .collect(),
            Category::Intermissions => vec![
                Box::new(IntermissionSuspend),
                Box::new(IntermissionPowerOff),
                Box::new(IntermissionShare),
            ],
            Category::Import => vec![Box::new(ImportStartupTrigger), Box::new(ImportSyncMetadata)],
            Category::Telemetry => {
                #[cfg(feature = "otel")]
                {
                    use super::kinds::telemetry::OtlpEndpoint;
                    let rows: Vec<Box<dyn SettingKind>> = vec![
                        Box::new(LoggingEnabled),
                        Box::new(LogLevel),
                        Box::new(OtlpEndpoint),
                    ];

                    #[cfg(all(feature = "test", feature = "kobo"))]
                    let mut rows = rows;
                    #[cfg(all(feature = "test", feature = "kobo"))]
                    {
                        use super::kinds::telemetry::EnableDbusLog;
                        use super::kinds::telemetry::EnableKernLog;
                        rows.push(Box::new(EnableKernLog));
                        rows.push(Box::new(EnableDbusLog));
                    }
                    rows
                }
                #[cfg(not(feature = "otel"))]
                {
                    let rows: Vec<Box<dyn SettingKind>> =
                        vec![Box::new(LoggingEnabled), Box::new(LogLevel)];

                    #[cfg(all(feature = "test", feature = "kobo"))]
                    let mut rows = rows;
                    #[cfg(all(feature = "test", feature = "kobo"))]
                    {
                        use super::kinds::telemetry::EnableDbusLog;
                        use super::kinds::telemetry::EnableKernLog;
                        rows.push(Box::new(EnableKernLog));
                        rows.push(Box::new(EnableDbusLog));
                    }
                    rows
                }
            }
        }
    }

    /// Returns all available categories.
    pub fn all() -> Vec<Category> {
        vec![
            Category::General,
            Category::Reader,
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
