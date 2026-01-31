use super::category::Category;
use super::category_navigation_bar::CategoryNavigationBar;
use crate::context::Context;
use crate::font::Fonts;
use crate::geom::{Point, Rectangle};
use crate::view::navigation::stack_navigation_bar::NavigationProvider;

/// Navigation provider for settings categories.
///
/// This provider implements the `NavigationProvider` trait for the flat
/// category hierarchy used in the settings editor. Since categories don't
/// have a parent-child relationship, this provider treats all categories
/// as independent root-level items.
#[derive(Default)]
pub struct SettingsCategoryProvider;

impl NavigationProvider for SettingsCategoryProvider {
    type LevelKey = Category;
    type LevelData = ();
    type Bar = CategoryNavigationBar;

    fn parent(&self, _current: &Self::LevelKey) -> Option<Self::LevelKey> {
        None
    }

    fn is_ancestor(&self, ancestor: &Self::LevelKey, descendant: &Self::LevelKey) -> bool {
        ancestor == descendant
    }

    fn is_root(&self, _key: &Self::LevelKey, _context: &Context) -> bool {
        true
    }

    fn fetch_level_data(&self, _key: &Self::LevelKey, _context: &mut Context) -> Self::LevelData {}

    /// Return 1 here, as the amount of categories fits on 1 line
    fn estimate_line_count(&self, _key: &Self::LevelKey, _data: &Self::LevelData) -> usize {
        1
    }

    fn create_bar(&self, rect: Rectangle, key: &Self::LevelKey) -> Self::Bar {
        CategoryNavigationBar::new(rect, *key)
    }

    fn bar_key(&self, bar: &Self::Bar) -> Self::LevelKey {
        bar.selected
    }

    fn update_bar(
        &self,
        bar: &mut Self::Bar,
        _data: &Self::LevelData,
        selected: &Self::LevelKey,
        _fonts: &mut Fonts,
    ) {
        bar.update_selection(*selected);
    }

    fn update_bar_selection(&self, bar: &mut Self::Bar, selected: &Self::LevelKey) {
        bar.update_selection(*selected);
    }

    fn resize_bar_by(&self, bar: &mut Self::Bar, delta_y: i32, fonts: &mut Fonts) -> i32 {
        bar.resize_by(delta_y, fonts)
    }

    fn shift_bar(&self, bar: &mut Self::Bar, delta: Point) {
        bar.shift(delta);
    }
}
