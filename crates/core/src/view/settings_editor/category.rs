use super::setting_row::Kind as RowKind;
use crate::context::Context;

/// Categories of settings available in the settings editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// General device settings (auto-suspend, keyboard layout, etc.)
    General,
    /// Library management settings
    Libraries,
    /// Intermission screen display settings
    Intermissions,
}

impl Category {
    /// Returns the display label for this category.
    pub fn label(&self) -> String {
        match self {
            Category::General => "General".to_string(),
            Category::Libraries => "Libraries".to_string(),
            Category::Intermissions => "Intermission Screens".to_string(),
        }
    }

    /// Returns the list of setting rows for this category.
    pub fn settings(&self, context: &Context) -> Vec<RowKind> {
        match self {
            Category::General => vec![
                RowKind::AutoShare,
                RowKind::AutoSuspend,
                RowKind::AutoPowerOff,
                RowKind::ButtonScheme,
                RowKind::KeyboardLayout,
                RowKind::SleepCover,
            ],
            Category::Libraries => (0..context.settings.libraries.len())
                .map(RowKind::Library)
                .collect(),
            Category::Intermissions => vec![
                RowKind::IntermissionSuspend,
                RowKind::IntermissionPowerOff,
                RowKind::IntermissionShare,
            ],
        }
    }

    /// Returns all available categories.
    pub fn all() -> Vec<Category> {
        vec![
            Category::General,
            Category::Libraries,
            Category::Intermissions,
        ]
    }
}
