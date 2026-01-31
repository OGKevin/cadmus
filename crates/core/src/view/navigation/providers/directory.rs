use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::Fonts;
use crate::geom::Point;
use crate::unit::scale_by_dpi;
use crate::view::home::directories_bar::DirectoriesBar;
use crate::view::navigation::stack_navigation_bar::NavigationProvider;
use crate::view::{View, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
pub struct DirectoryNavigationProvider;

impl DirectoryNavigationProvider {
    #[inline]
    fn guess_bar_size(dirs: &BTreeSet<PathBuf>) -> usize {
        (dirs.iter().map(|dir| dir.as_os_str().len()).sum::<usize>() / 300).clamp(1, 4)
    }
}

impl NavigationProvider for DirectoryNavigationProvider {
    type LevelKey = PathBuf;
    type LevelData = BTreeSet<PathBuf>;
    type Bar = DirectoriesBar;

    fn selected_leaf_key(&self, selected: &Self::LevelKey) -> Self::LevelKey {
        selected.clone()
    }

    /// Determines the appropriate directory level for navigation bar traversal.
    ///
    /// If the selected directory has no subdirectories and is not the library home,
    /// returns the parent directory to ensure the navigation bar displays a level
    /// with navigable content. Otherwise, returns the selected directory itself.
    ///
    /// # Arguments
    ///
    /// * `selected` - The currently selected directory path
    /// * `context` - Application context containing library information
    ///
    /// # Returns
    ///
    /// The directory path to use for bar traversal navigation
    fn leaf_for_bar_traversal(
        &self,
        selected: &Self::LevelKey,
        context: &Context,
    ) -> Self::LevelKey {
        let (_, dirs) = context.library.list(selected, None, true);
        if dirs.is_empty() && *selected != context.library.home {
            selected
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| selected.clone())
        } else {
            selected.clone()
        }
    }

    fn parent(&self, current: &Self::LevelKey) -> Option<Self::LevelKey> {
        current.parent().map(|p| p.to_path_buf())
    }

    fn is_ancestor(&self, ancestor: &Self::LevelKey, descendant: &Self::LevelKey) -> bool {
        descendant.starts_with(ancestor)
    }

    fn is_root(&self, key: &Self::LevelKey, context: &Context) -> bool {
        *key == context.library.home
    }

    fn fetch_level_data(&self, key: &Self::LevelKey, context: &mut Context) -> Self::LevelData {
        let (_, dirs) = context.library.list(key, None, true);
        dirs
    }

    fn estimate_line_count(&self, _key: &Self::LevelKey, data: &Self::LevelData) -> usize {
        Self::guess_bar_size(data)
    }

    fn create_bar(&self, rect: crate::geom::Rectangle, key: &Self::LevelKey) -> Self::Bar {
        DirectoriesBar::new(rect, key)
    }

    fn bar_key(&self, bar: &Self::Bar) -> Self::LevelKey {
        bar.path.clone()
    }

    fn update_bar(
        &self,
        bar: &mut Self::Bar,
        data: &Self::LevelData,
        selected: &Self::LevelKey,
        fonts: &mut Fonts,
    ) {
        bar.update_content(data, Path::new(selected), fonts);
    }

    fn update_bar_selection(&self, bar: &mut Self::Bar, selected: &Self::LevelKey) {
        bar.update_selected(Path::new(selected));
    }

    fn resize_bar_by(&self, bar: &mut Self::Bar, delta_y: i32, fonts: &mut Fonts) -> i32 {
        let rectangle = *bar.rect();
        let dpi = CURRENT_DEVICE.dpi;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let min_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32 - thickness;

        let y_max = (rectangle.max.y + delta_y).max(rectangle.min.y + min_height);
        let resized = y_max - rectangle.max.y;

        bar.rect_mut().max.y = y_max;

        let dirs = bar.dirs();
        let path = bar.path.clone();
        bar.update_content(&dirs, path.as_path(), fonts);

        resized
    }

    fn shift_bar(&self, bar: &mut Self::Bar, delta: Point) {
        bar.shift(delta);
    }
}
