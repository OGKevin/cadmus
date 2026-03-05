use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::Fonts;
use crate::geom::Point;
use crate::unit::scale_by_dpi;
use crate::view::home::directories_bar::DirectoriesBar;
use crate::view::navigation::stack_navigation_bar::NavigationProvider;
use crate::view::{View, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The source of directory listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceType {
    /// Plain filesystem - shows all directories.
    Filesystem,
    /// Library-aware - respects library filtering rules.
    Library,
}

/// A navigation provider that traverses directory structures.
///
/// This provider can use either plain filesystem operations or library-aware
/// filtering, depending on the use case:
/// - Filesystem mode: Shows all directories (for FileChooser)
/// - Library mode: Respects library settings like hidden file filtering (for Home view)
///
/// # Examples
///
/// Create a filesystem provider for browsing all directories:
///
/// ```
/// use std::path::PathBuf;
/// use cadmus_core::view::navigation::providers::directory::DirectoryNavigationProvider;
///
/// let provider = DirectoryNavigationProvider::filesystem(PathBuf::from("/home/user"));
/// ```
///
/// Create a library provider for filtered navigation:
///
/// ```
/// use std::path::PathBuf;
/// use cadmus_core::view::navigation::providers::directory::DirectoryNavigationProvider;
///
/// let provider = DirectoryNavigationProvider::library(PathBuf::from("/home/user/books"));
/// ```
#[derive(Debug, Clone)]
pub struct DirectoryNavigationProvider {
    root: PathBuf,
    source_type: SourceType,
}

impl DirectoryNavigationProvider {
    /// Creates a provider for plain filesystem navigation.
    ///
    /// This shows all directories without any filtering, suitable for
    /// file chooser dialogs that need to browse the entire filesystem.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use cadmus_core::view::navigation::providers::directory::DirectoryNavigationProvider;
    ///
    /// let provider = DirectoryNavigationProvider::filesystem(PathBuf::from("/home/user"));
    /// ```
    pub fn filesystem(root: PathBuf) -> Self {
        Self {
            root,
            source_type: SourceType::Filesystem,
        }
    }

    /// Creates a provider for library-aware navigation.
    ///
    /// This respects library settings like hidden file filtering,
    /// suitable for the Home view navigation.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use cadmus_core::view::navigation::providers::directory::DirectoryNavigationProvider;
    ///
    /// let provider = DirectoryNavigationProvider::library(PathBuf::from("/home/user/books"));
    /// ```
    pub fn library(root: PathBuf) -> Self {
        Self {
            root,
            source_type: SourceType::Library,
        }
    }

    /// Updates the root directory for this provider.
    pub fn set_root(&mut self, root: PathBuf) {
        self.root = root;
    }

    /// Lists directories using the configured source.
    #[inline]
    fn list_directories(&self, path: &Path, context: &Context) -> BTreeSet<PathBuf> {
        match self.source_type {
            SourceType::Filesystem => self.list_filesystem_dirs(path),
            SourceType::Library => self.list_library_dirs(path, context),
        }
    }

    /// Lists directories from the filesystem without filtering.
    #[inline]
    fn list_filesystem_dirs(&self, path: &Path) -> BTreeSet<PathBuf> {
        let mut dirs = BTreeSet::new();

        if !path.is_dir() {
            return dirs;
        }

        let read_dir = match fs::read_dir(path) {
            Ok(rd) => rd,
            Err(_) => return dirs,
        };

        for entry in read_dir.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_dir() {
                    dirs.insert(entry.path());
                }
            }
        }

        dirs
    }

    /// Lists directories using the library's filtering rules.
    #[inline]
    fn list_library_dirs(&self, path: &Path, context: &Context) -> BTreeSet<PathBuf> {
        context.library.list(path, None, true).1
    }

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

    fn leaf_for_bar_traversal(
        &self,
        selected: &Self::LevelKey,
        context: &Context,
    ) -> Self::LevelKey {
        let dirs = self.list_directories(selected, context);

        if dirs.is_empty() && *selected != self.root {
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

    fn is_root(&self, key: &Self::LevelKey, _context: &Context) -> bool {
        *key == self.root
    }

    fn fetch_level_data(&self, key: &Self::LevelKey, context: &mut Context) -> Self::LevelData {
        self.list_directories(key, context)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_directory_structure() -> TempDir {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        fs::create_dir(root.join("dir_a")).unwrap();
        fs::create_dir(root.join("dir_b")).unwrap();
        fs::create_dir(root.join("dir_c")).unwrap();
        fs::create_dir(root.join("dir_a").join("nested")).unwrap();

        let mut file = fs::File::create(root.join("file.txt")).unwrap();
        file.write_all(b"test").unwrap();

        temp_dir
    }

    #[test]
    fn filesystem_source_lists_all_directories() {
        let temp_dir = create_test_directory_structure();
        let root = temp_dir.path().to_path_buf();
        let provider = DirectoryNavigationProvider::filesystem(root.clone());
        let context = create_test_context();

        let dirs = provider.list_directories(&root, &context);

        assert_eq!(dirs.len(), 3);
        assert!(dirs.contains(&root.join("dir_a")));
        assert!(dirs.contains(&root.join("dir_b")));
        assert!(dirs.contains(&root.join("dir_c")));
    }

    #[test]
    fn filesystem_source_returns_empty_for_nonexistent_path() {
        let root = PathBuf::from("/nonexistent/path");
        let provider = DirectoryNavigationProvider::filesystem(root.clone());
        let context = create_test_context();

        let dirs = provider.list_directories(&root, &context);

        assert!(dirs.is_empty());
    }

    #[test]
    fn filesystem_source_returns_empty_for_file() {
        let temp_dir = create_test_directory_structure();
        let root = temp_dir.path().to_path_buf();
        let provider = DirectoryNavigationProvider::filesystem(root.clone());
        let context = create_test_context();

        let dirs = provider.list_directories(&root.join("file.txt"), &context);

        assert!(dirs.is_empty());
    }

    #[test]
    fn is_root_returns_true_for_root() {
        let temp_dir = create_test_directory_structure();
        let root = temp_dir.path().to_path_buf();
        let provider = DirectoryNavigationProvider::filesystem(root.clone());

        assert!(provider.is_root(&root, &create_test_context()));
    }

    #[test]
    fn is_root_returns_false_for_non_root() {
        let temp_dir = create_test_directory_structure();
        let root = temp_dir.path().to_path_buf();
        let provider = DirectoryNavigationProvider::filesystem(root.clone());

        let subdir = root.join("dir_a");
        assert!(!provider.is_root(&subdir, &create_test_context()));
    }

    #[test]
    fn fetch_level_data_returns_directories() {
        let temp_dir = create_test_directory_structure();
        let root = temp_dir.path().to_path_buf();
        let provider = DirectoryNavigationProvider::filesystem(root.clone());

        let dirs = provider.fetch_level_data(&root, &mut create_test_context());

        assert_eq!(dirs.len(), 3);
    }

    #[test]
    fn leaf_for_bar_traversal_returns_selected_when_has_subdirs() {
        let temp_dir = create_test_directory_structure();
        let root = temp_dir.path().to_path_buf();
        let provider = DirectoryNavigationProvider::filesystem(root.clone());

        let selected = root.join("dir_a");
        let result = provider.leaf_for_bar_traversal(&selected, &create_test_context());

        assert_eq!(result, selected);
    }

    #[test]
    fn leaf_for_bar_traversal_returns_parent_when_empty() {
        let temp_dir = create_test_directory_structure();
        let root = temp_dir.path().to_path_buf();
        let provider = DirectoryNavigationProvider::filesystem(root.clone());

        let selected = root.join("dir_a").join("nested");
        let result = provider.leaf_for_bar_traversal(&selected, &create_test_context());

        assert_eq!(result, root.join("dir_a"));
    }

    #[test]
    fn leaf_for_bar_traversal_returns_root_when_root_is_empty() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let provider = DirectoryNavigationProvider::filesystem(root.clone());

        let result = provider.leaf_for_bar_traversal(&root, &create_test_context());

        assert_eq!(result, root);
    }
}
