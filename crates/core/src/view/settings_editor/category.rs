use super::setting_row::Kind as RowKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    General,
}

impl Category {
    pub fn label(&self) -> String {
        match self {
            Category::General => "General".to_string(),
        }
    }

    pub fn settings(&self) -> Vec<RowKind> {
        match self {
            Category::General => vec![
                RowKind::AutoShare,
                RowKind::ButtonScheme,
                RowKind::KeyboardLayout,
                RowKind::SleepCover,
            ],
        }
    }

    pub fn all() -> Vec<Category> {
        vec![Category::General]
    }
}
