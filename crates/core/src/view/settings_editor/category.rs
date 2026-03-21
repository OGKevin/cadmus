use super::setting_row::Kind as RowKind;
use crate::context::Context;

/// Categories of settings available in the settings editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// General device settings (auto-suspend, keyboard layout, etc.)
    General,
    /// Reader behavior settings (finished action, etc.)
    Reader,
    /// Library management settings
    Libraries,
    /// Intermission screen display settings
    Intermissions,
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
            Category::Telemetry => "Telemetry".to_string(),
        }
    }

    /// Returns the list of setting rows for this category.
    pub fn settings(&self, context: &Context) -> Vec<RowKind> {
        match self {
            Category::General => vec![
                RowKind::Locale,
                RowKind::AutoShare,
                RowKind::AutoSuspend,
                RowKind::AutoPowerOff,
                RowKind::ButtonScheme,
                RowKind::KeyboardLayout,
                RowKind::SleepCover,
                RowKind::SettingsRetention,
            ],
            Category::Reader => vec![RowKind::FinishedAction],
            Category::Libraries => (0..context.settings.libraries.len())
                .map(RowKind::Library)
                .collect(),
            Category::Intermissions => vec![
                RowKind::IntermissionSuspend,
                RowKind::IntermissionPowerOff,
                RowKind::IntermissionShare,
            ],
            Category::Telemetry => {
                #[cfg(feature = "otel")]
                {
                    let rows = vec![
                        RowKind::LoggingEnabled,
                        RowKind::LogLevel,
                        RowKind::OtlpEndpoint,
                        #[cfg(feature = "test")]
                        RowKind::EnableKernLog,
                    ];
                    rows
                }
                #[cfg(not(feature = "otel"))]
                {
                    vec![
                        RowKind::LoggingEnabled,
                        RowKind::LogLevel,
                        #[cfg(feature = "test")]
                        RowKind::EnableKernLog,
                    ]
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
            Category::Telemetry,
        ]
    }

    /// Returns the number of categories.
    pub fn count() -> usize {
        Self::all().len()
    }
}
