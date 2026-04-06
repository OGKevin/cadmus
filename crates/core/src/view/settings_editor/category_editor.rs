use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, Rectangle};
use crate::settings::{LibrarySettings, Settings};
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::filler::Filler;
use crate::view::menu::{Menu, MenuKind};
use crate::view::toggleable_keyboard::ToggleableKeyboard;
use crate::view::{
    Bus, EntryId, EntryKind, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, ID_FEEDER,
    SMALL_BAR_HEIGHT, THICKNESS_MEDIUM,
};

use super::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};
use super::category::Category;
use super::kinds::library::LibraryInfo;
use super::library_editor::LibraryEditor;
use super::setting_row::SettingRow;
use std::path::PathBuf;

/// A view for editing category-specific settings.
///
/// The `CategoryEditor` manages the UI for editing settings within a specific category
/// (e.g., Libraries, Intermissions, etc.). It displays setting rows, handles user interactions,
/// and manages child views such as keyboards and input fields.
///
/// All settings changes are applied immediately to `context.settings`, providing instant
/// feedback without requiring explicit validation.
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - The rectangular area occupied by this view
/// * `children` - Child views managed by this editor. The structure is:
///   1. Content background filler (index 0)
///   2. Setting rows (indices from `first_row_index` onwards)
///   3. BottomSeparator (variable index, only for Libraries category)
///   4. BottomBar (variable index, only for Libraries category)
///   5. ToggleableKeyboard (at index `keyboard_index`)
///   6. Plus optional overlay views like LibraryEditor and NamedInput fields
/// * `category` - The settings category being edited
/// * `content_rect` - The rectangular area where setting rows are displayed
/// * `row_height` - The height of each setting row
/// * `focus` - Currently focused child view, if any
/// * `first_row_index` - Index in the children vector where setting rows begin (after structural elements)
/// * `keyboard_index` - Index of the keyboard child view in the children vector
pub struct CategoryEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    category: Category,
    content_rect: Rectangle,
    row_height: i32,
    focus: Option<ViewId>,
    first_row_index: usize,
    keyboard_index: usize,
}

impl CategoryEditor {
    pub fn new(
        rect: Rectangle,
        category: Category,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> CategoryEditor {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        let (bar_height, separator_top_half, separator_bottom_half) = Self::calculate_dimensions();

        let mut content_rect = rect![rect.min.x, rect.min.y, rect.max.x, rect.max.y];

        if matches!(category, Category::Libraries) {
            content_rect = rect![
                rect.min.x,
                rect.min.y,
                rect.max.x,
                rect.max.y - bar_height - separator_top_half
            ];
        }

        let content_rect = content_rect;

        let background = Filler::new(content_rect, WHITE);
        children.push(Box::new(background) as Box<dyn View>);

        let first_row_index = children.len();

        let row_height = scale_by_dpi(SMALL_BAR_HEIGHT, CURRENT_DEVICE.dpi) as i32;
        let setting_kinds = category.settings(context);
        let mut current_y = content_rect.min.y;

        for kind in setting_kinds {
            let row_rect = rect![
                content_rect.min.x,
                current_y,
                content_rect.max.x,
                current_y + row_height
            ];
            children.push(Self::build_setting_row(
                kind,
                row_rect,
                &context.settings,
                &mut context.fonts,
            ));
            current_y += row_height;
        }

        if matches!(category, Category::Libraries) {
            children.push(Self::build_bottom_separator(
                rect,
                bar_height,
                separator_top_half,
                separator_bottom_half,
            ));

            children.push(Self::build_bottom_bar(
                rect,
                bar_height,
                separator_bottom_half,
                category,
            ));
        }

        let keyboard = ToggleableKeyboard::new(rect, true);
        children.push(Box::new(keyboard) as Box<dyn View>);

        let keyboard_index = children.len() - 1;

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        CategoryEditor {
            id,
            rect,
            children,
            category,
            content_rect,
            row_height,
            focus: None,
            first_row_index,
            keyboard_index,
        }
    }

    pub fn category(&self) -> Category {
        self.category
    }

    #[inline]
    fn calculate_dimensions() -> (i32, i32, i32) {
        let dpi = CURRENT_DEVICE.dpi;
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;

        let separator_thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (separator_top_half, separator_bottom_half) = halves(separator_thickness);
        let bar_height = small_height;

        (bar_height, separator_top_half, separator_bottom_half)
    }

    #[inline]
    fn build_setting_row(
        kind: Box<dyn super::kinds::SettingKind>,
        row_rect: Rectangle,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> Box<dyn View> {
        let setting_row = SettingRow::new(kind, row_rect, settings, fonts);
        Box::new(setting_row) as Box<dyn View>
    }

    #[inline]
    fn build_bottom_separator(
        rect: Rectangle,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        let separator = Filler::new(
            rect![
                rect.min.x,
                rect.max.y - bar_height - separator_top_half,
                rect.max.x,
                rect.max.y - bar_height + separator_bottom_half
            ],
            BLACK,
        );
        Box::new(separator) as Box<dyn View>
    }

    #[inline]
    fn build_bottom_bar(
        rect: Rectangle,
        bar_height: i32,
        separator_bottom_half: i32,
        category: Category,
    ) -> Box<dyn View> {
        let bottom_bar_rect = rect![
            rect.min.x,
            rect.max.y - bar_height + separator_bottom_half,
            rect.max.x,
            rect.max.y
        ];

        match category {
            Category::Libraries => Box::new(SettingsEditorBottomBar::new(
                bottom_bar_rect,
                BottomBarVariant::SingleButton {
                    event: Event::AddLibrary,
                    icon: "plus",
                },
            )),
            _ => unreachable!("These categories have no bottom bar"),
        }
    }

    /// Rebuilds the library rows in the UI after a library is added, removed, or modified.
    ///
    /// This method removes the old library rows and inserts new ones based on the current
    /// state of `context.settings.libraries`. It only operates when the current category is
    /// `Category::Libraries`.
    ///
    /// # Example
    ///
    /// If we have 3 structural children + 2 library rows + keyboard:
    /// ```txt
    /// Before:  [TopBar, TopSep, BgFiller, LibRow0, LibRow1, BottomSep, BottomBar, Keyboard]
    ///           indices: 0      1        2        3        4         5          6        7
    /// ```
    ///
    /// After adding a library (original_count=2, now 3 libraries):
    /// ```txt
    /// Removal phase:    [TopBar, TopSep, BgFiller, BottomSep, BottomBar, Keyboard]
    ///                    Remove indices 3,4 (2 rows)
    ///
    /// Insertion phase:  [TopBar, TopSep, BgFiller, LibRow0, LibRow1, LibRow2, BottomSep, BottomBar, Keyboard]
    ///                    Insert at indices 3,4,5
    /// ```
    ///
    /// # Arguments
    ///
    /// * `rq` - The render queue to add render updates to
    /// * `context` - The application context containing settings
    /// * `original_count` - The original number of library rows before the change. If `None`,
    ///   uses the current library count. This is used to determine how many old rows to remove.
    #[inline]
    fn rebuild_library_rows(
        &mut self,
        rq: &mut RenderQueue,
        context: &mut Context,
        original_count: Option<usize>,
    ) {
        if self.category != Category::Libraries {
            return;
        }

        let num_libraries = context.settings.libraries.len();
        let rows_to_remove = original_count.unwrap_or(num_libraries);

        let first_row_index = self.first_row_index;

        for _ in 0..rows_to_remove {
            if first_row_index < self.children.len() {
                self.children.remove(first_row_index);
            }
        }

        let mut current_y = self.content_rect.min.y;
        let mut new_rows = Vec::new();

        for i in 0..num_libraries {
            let row_rect = rect![
                self.content_rect.min.x,
                current_y,
                self.content_rect.max.x,
                current_y + self.row_height
            ];

            let setting_row = SettingRow::new(
                Box::new(LibraryInfo(i)),
                row_rect,
                &context.settings,
                &mut context.fonts,
            );

            new_rows.push(Box::new(setting_row) as Box<dyn View>);
            current_y += self.row_height;
        }

        for (offset, row) in new_rows.into_iter().enumerate() {
            self.children.insert(first_row_index + offset, row);
        }

        self.keyboard_index = self.children.len() - 1;

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
    }

    #[inline]
    fn toggle_keyboard(
        &mut self,
        visible: bool,
        _id: Option<ViewId>,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        let keyboard = self.children[self.keyboard_index]
            .downcast_mut::<ToggleableKeyboard>()
            .expect("keyboard_index points to non-ToggleableKeyboard view");
        keyboard.set_visible(visible, hub, rq, context);
    }

    #[inline]
    fn handle_focus_event(
        &mut self,
        view_id: &Option<ViewId>,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if self.focus != *view_id {
            self.focus = *view_id;
            if view_id.is_some() {
                self.toggle_keyboard(true, *view_id, hub, rq, context);
            } else {
                self.toggle_keyboard(false, None, hub, rq, context);
            }
        }
        true
    }

    #[inline]
    fn handle_submenu_event(
        &mut self,
        rect: &Rectangle,
        entries: &[EntryKind],
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let menu = Menu::new(
            *rect,
            ViewId::SettingsValueMenu,
            MenuKind::Contextual,
            entries.to_vec(),
            context,
        );

        rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
        self.children.push(Box::new(menu));

        true
    }

    #[inline]
    fn handle_delete_library(
        &mut self,
        index: usize,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if index < context.settings.libraries.len() {
            let original_count = context.settings.libraries.len();
            context.settings.libraries.remove(index);

            self.rebuild_library_rows(rq, context, Some(original_count));
        }

        if let Some(menu_index) = locate_by_id(self, ViewId::SettingsValueMenu) {
            self.children.remove(menu_index);
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }

        true
    }

    /// Handles the `AddLibrary` event by creating a new library and opening an editor overlay.
    ///
    /// This function:
    /// 1. Creates a new `LibrarySettings` with default values
    /// 2. Adds it immediately to `context.settings.libraries`
    /// 3. Rebuilds the library rows to display the new library in the list
    /// 4. Opens a `LibraryEditor` overlay so the user can immediately configure the new library
    ///
    /// The `LibraryEditor` is pushed to the end of the children array, after the keyboard.
    /// This means `keyboard_index` remains valid and continues to correctly point to the keyboard,
    /// while the `LibraryEditor` becomes the new last child.
    #[inline]
    fn handle_add_library_event(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let library = LibrarySettings {
            name: "untitled".to_string(),
            path: PathBuf::from("/mnt/onboard"),
            ..Default::default()
        };

        let library_editor = LibraryEditor::new(
            self.rect,
            context.settings.libraries.len(),
            library,
            hub,
            rq,
            context,
        );
        self.children.push(Box::new(library_editor));
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        true
    }

    /// Handles the `EditLibrary` event by opening a `LibraryEditor` overlay for the specified library.
    ///
    /// This function creates a `LibraryEditor` view that allows the user to modify an existing
    /// library's settings (name, path, mode, etc.). The editor is pushed as a child view,
    /// creating an overlay on top of the category editor. The `LibraryEditor` is pushed to the
    /// end of the children array, after the keyboard, so `keyboard_index` remains valid.
    #[inline]
    fn handle_edit_library_event(
        &mut self,
        index: usize,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Some(library) = context.settings.libraries.get(index).cloned() {
            let library_editor = LibraryEditor::new(self.rect, index, library, hub, rq, context);
            self.children.push(Box::new(library_editor));
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }
        true
    }

    #[inline]
    fn handle_update_library_event(
        &mut self,
        index: usize,
        library: &LibrarySettings,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if index < context.settings.libraries.len() {
            context.settings.libraries[index] = library.clone();
        } else if index == context.settings.libraries.len() {
            context.settings.libraries.push(library.clone());
        } else {
            return true;
        }

        self.rebuild_library_rows(rq, context, None);
        true
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn handle_open_named_input(
        &mut self,
        view_id: ViewId,
        label: String,
        max_chars: usize,
        initial_text: String,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let mut named_input =
            crate::view::named_input::NamedInput::new(label, view_id, view_id, max_chars, context);
        named_input.set_text(&initial_text, rq, context);
        self.children.push(Box::new(named_input));
        hub.send(Event::Focus(Some(view_id))).ok();
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    fn handle_close_view_event(
        &mut self,
        view_id: &ViewId,
        hub: &Hub,
        rq: &mut RenderQueue,
    ) -> bool {
        match view_id {
            ViewId::AutoSuspendInput
            | ViewId::AutoPowerOffInput
            | ViewId::SettingsRetentionInput
            | ViewId::SettingsValueMenu
            | ViewId::LibraryEditor => {
                if let Some(index) = locate_by_id(self, *view_id) {
                    let input_rect = *self.children[index].rect();
                    self.children.remove(index);
                    rq.add(RenderData::expose(input_rect, UpdateMode::Gui));
                }
                hub.send(Event::Focus(None)).ok();
                true
            }
            #[cfg(feature = "otel")]
            ViewId::OtlpEndpointInput => {
                if let Some(index) = locate_by_id(self, *view_id) {
                    let input_rect = *self.children[index].rect();
                    self.children.remove(index);
                    rq.add(RenderData::expose(input_rect, UpdateMode::Gui));
                }
                hub.send(Event::Focus(None)).ok();
                true
            }
            _ => false,
        }
    }
}

impl View for CategoryEditor {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub, _bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::Focus(view_id) => self.handle_focus_event(view_id, hub, rq, context),
            Event::SubMenu(rect, ref entries) => {
                self.handle_submenu_event(rect, entries, rq, context)
            }
            Event::Select(EntryId::DeleteLibrary(index)) => {
                self.handle_delete_library(*index, rq, context)
            }
            Event::AddLibrary => self.handle_add_library_event(hub, rq, context),
            Event::EditLibrary(index) => self.handle_edit_library_event(*index, hub, rq, context),
            Event::UpdateLibrary(index, ref library) => {
                self.handle_update_library_event(*index, library, rq, context)
            }
            Event::OpenNamedInput {
                view_id,
                ref label,
                max_chars,
                ref initial_text,
            } => self.handle_open_named_input(
                *view_id,
                label.clone(),
                *max_chars,
                initial_text.clone(),
                hub,
                rq,
                context,
            ),
            Event::Close(view_id) => self.handle_close_view_event(view_id, hub, rq),
            _ => false,
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut crate::font::Fonts) {
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }

    fn is_background(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::geom::Point;
    use crate::gesture::GestureEvent;
    use crate::settings::Settings;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    fn create_test_settings_with_libraries(count: usize) -> Settings {
        let mut settings = Settings::default();
        settings.libraries.clear();
        for i in 0..count {
            settings.libraries.push(LibrarySettings {
                name: format!("Library {}", i),
                path: PathBuf::from(format!("/mnt/onboard/lib{}", i)),
                ..Default::default()
            });
        }
        settings
    }

    fn create_test_category_editor_with_context(context: &mut Context) -> CategoryEditor {
        let rect = rect![0, 0, 600, 800];
        let mut rq = RenderQueue::new();

        CategoryEditor::new(rect, Category::Libraries, &mut rq, context)
    }

    #[test]
    fn test_add_library_event() {
        let mut context = create_test_context();
        context.settings = Settings::default();
        context.settings.libraries.clear();
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(context.settings.libraries.len(), 0);
        let initial_children_count = editor.children.len();

        let handled =
            editor.handle_event(&Event::AddLibrary, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(context.settings.libraries.len(), 0);

        assert_eq!(
            editor.children.len(),
            initial_children_count + 1,
            "Expected +1: one library editor"
        );
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_add_library_preserves_structural_children() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(2);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(context.settings.libraries.len(), 2);
        let initial_children_count = editor.children.len();

        let handled =
            editor.handle_event(&Event::AddLibrary, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(context.settings.libraries.len(), 2);

        assert_eq!(
            editor.children.len(),
            initial_children_count + 1,
            "Expected children count to increase by 1: one library editor"
        );

        assert_eq!(
            // minus 2 to account for the newly added library editor
            editor.keyboard_index,
            editor.children.len() - 2,
            "keyboard_index should point to the last child (the keyboard)"
        );

        assert!(
            editor.keyboard_index < editor.children.len(),
            "keyboard_index out of bounds - structural children were likely removed incorrectly"
        );

        let keyboard_still_exists = editor
            .children
            .iter()
            .any(|child| child.downcast_ref::<ToggleableKeyboard>().is_some());

        assert!(
            keyboard_still_exists,
            "ToggleableKeyboard view should still exist in children after adding library"
        );
    }

    #[test]
    fn test_delete_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(2);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(context.settings.libraries.len(), 2);
        assert_eq!(context.settings.libraries[0].name, "Library 0");
        assert_eq!(context.settings.libraries[1].name, "Library 1");

        let row_y = editor.content_rect.min.y + (editor.row_height / 2);
        let point = Point::new(editor.content_rect.min.x + 10, row_y);

        editor.handle_event(
            &Event::Gesture(GestureEvent::HoldFingerShort(point, 0)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        rq = RenderQueue::new();

        let handled = editor.handle_event(
            &Event::Select(EntryId::DeleteLibrary(0)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Library 1");

        assert!(!rq.is_empty());
    }

    #[test]
    fn test_update_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Library 0");

        let updated_library = LibrarySettings {
            name: "Updated Library".to_string(),
            path: PathBuf::from("/mnt/onboard/updated"),
            ..Default::default()
        };

        let handled = editor.handle_event(
            &Event::UpdateLibrary(0, Box::new(updated_library.clone())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Updated Library");
        assert_eq!(
            context.settings.libraries[0].path,
            PathBuf::from("/mnt/onboard/updated")
        );
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_edit_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_children_count = editor.children.len();

        let handled = editor.handle_event(
            &Event::EditLibrary(0),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(editor.children.len(), initial_children_count + 1);
        assert!(!rq.is_empty());
    }

    fn create_test_intermissions_category_editor(context: &mut Context) -> CategoryEditor {
        let rect = rect![0, 0, 600, 800];
        let mut rq = RenderQueue::new();

        CategoryEditor::new(rect, Category::Intermissions, &mut rq, context)
    }

    #[test]
    fn test_set_intermission_logo() {
        use crate::settings::{IntermKind, IntermissionDisplay};

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled = crate::view::handle_event(
            &mut editor,
            &Event::Select(EntryId::SetIntermission(
                IntermKind::Suspend,
                IntermissionDisplay::Logo,
            )),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert!(matches!(
            context.settings.intermissions[IntermKind::Suspend],
            IntermissionDisplay::Logo
        ));
    }

    #[test]
    fn test_set_intermission_cover() {
        use crate::settings::{IntermKind, IntermissionDisplay};

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled = crate::view::handle_event(
            &mut editor,
            &Event::Select(EntryId::SetIntermission(
                IntermKind::PowerOff,
                IntermissionDisplay::Cover,
            )),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert!(matches!(
            context.settings.intermissions[IntermKind::PowerOff],
            IntermissionDisplay::Cover
        ));
    }
}
