use super::setting_row::Kind as RowKind;
use crate::context::Context;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    General,
    Libraries,
}

impl Category {
    pub fn label(&self) -> String {
        match self {
            Category::General => "General".to_string(),
            Category::Libraries => "Libraries".to_string(),
        }
    }

    pub fn settings(&self, context: &Context) -> Vec<RowKind> {
        match self {
            Category::General => vec![
                RowKind::AutoShare,
                RowKind::ButtonScheme,
                RowKind::KeyboardLayout,
                RowKind::SleepCover,
            ],
            Category::Libraries => (0..context.settings.libraries.len())
                .map(|i| RowKind::Library(i))
                .collect(),
        }
    }

    pub fn all() -> Vec<Category> {
        vec![Category::General, Category::Libraries]
    }
}
