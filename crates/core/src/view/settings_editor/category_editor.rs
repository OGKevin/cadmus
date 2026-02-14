use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, Rectangle};
use crate::gesture::GestureEvent;
use crate::settings::{ButtonScheme, LibraryMode, LibrarySettings, Settings};
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::filler::Filler;
use crate::view::menu::{Menu, MenuKind};
use crate::view::toggleable_keyboard::ToggleableKeyboard;
use crate::view::{
    Bus, EntryId, EntryKind, Event, Hub, Id, RenderData, RenderQueue, ToggleEvent, View, ViewId,
    ID_FEEDER, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM,
};

use super::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};
use super::category::Category;
use super::library_editor::LibraryEditor;
use super::setting_row::{Kind as RowKind, SettingRow};
use crate::view::file_chooser::{FileChooser, SelectionMode};
use crate::view::settings_editor::ToggleSettings;
use std::path::PathBuf;

/// A view for editing category-specific settings.
///
/// The `CategoryEditor` manages the UI for editing settings within a specific category
/// (e.g., Libraries, Intermissions, etc.). It displays setting rows, handles user interactions,
/// and manages child views such as keyboards, input fields, and file choosers.
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
///   6. Plus optional overlay views like LibraryEditor, FileChooser, Menu, and NamedInput fields
/// * `category` - The settings category being edited
/// * `content_rect` - The rectangular area where setting rows are displayed
/// * `row_height` - The height of each setting row
/// * `focus` - Currently focused child view, if any
/// * `first_row_index` - Index in the children vector where setting rows begin (after structural elements)
/// * `keyboard_index` - Index of the keyboard child view in the children vector
/// * `active_intermission_edit` - Tracks which intermission type is currently being edited via file chooser
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
    active_intermission_edit: Option<crate::settings::IntermKind>,
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
            active_intermission_edit: None,
        }
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
        kind: RowKind,
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
                RowKind::Library(i),
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

    /// Refreshes all SettingValue views to reflect current state in context.settings.
    ///
    /// This method iterates through all child SettingRow views and their nested SettingValue
    /// children, calling their refresh_from_context method to update displayed values.
    /// Should be called after any setting is modified to ensure UI reflects the changes.
    #[inline]
    fn refresh_setting_values(&mut self, context: &Context, rq: &mut RenderQueue) {
        use super::setting_row::SettingRow;
        use super::setting_value::SettingValue;

        for child in &mut self.children {
            if let Some(row) = child.as_any_mut().downcast_mut::<SettingRow>() {
                for grandchild in row.children_mut() {
                    if let Some(value) = grandchild.as_any_mut().downcast_mut::<SettingValue>() {
                        value.refresh_from_context(context, rq);
                    }
                }
            }
        }
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

    /// Handles a short hold finger gesture to show a context menu for deleting libraries.
    #[inline]
    fn handle_hold_finger_short(
        &mut self,
        point: &crate::geom::Point,
        bus: &mut Bus,
        context: &Context,
    ) -> bool {
        if self.category != Category::Libraries {
            return false;
        }

        if !self.content_rect.includes(*point) {
            return false;
        }

        let row_index = (point.y - self.content_rect.min.y) / self.row_height;
        let library_index = row_index as usize;

        if library_index < context.settings.libraries.len() {
            let row_y = self.content_rect.min.y + (row_index * self.row_height);
            let row_rect = rect![
                self.content_rect.min.x,
                row_y,
                self.content_rect.max.x,
                row_y + self.row_height
            ];

            let entries = vec![EntryKind::Command(
                "Delete".to_string(),
                EntryId::DeleteLibrary(library_index),
            )];

            bus.push_back(Event::SubMenu(row_rect, entries));
            return true;
        }

        false
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
    fn handle_set_keyboard_layout(
        &mut self,
        layout: &str,
        _evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        context.settings.keyboard_layout = layout.to_string();
        self.refresh_setting_values(context, rq);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    fn handle_toggle_sleep_cover(
        &mut self,
        _evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        context.settings.sleep_cover = !context.settings.sleep_cover;
        self.refresh_setting_values(context, rq);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    fn handle_toggle_auto_share(
        &mut self,
        _evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        context.settings.auto_share = !context.settings.auto_share;
        self.refresh_setting_values(context, rq);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    fn handle_edit_auto_suspend(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let mut suspend_input = crate::view::named_input::NamedInput::new(
            "Auto Suspend (minutes, 0 = never)".to_string(),
            ViewId::AutoSuspendInput,
            ViewId::AutoSuspendInput,
            10,
            context,
        );
        let text = if context.settings.auto_suspend == 0.0 {
            "0".to_string()
        } else {
            format!("{:.1}", context.settings.auto_suspend)
        };

        suspend_input.set_text(&text, rq, context);

        self.children.push(Box::new(suspend_input));
        hub.send(Event::Focus(Some(ViewId::AutoSuspendInput))).ok();

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        true
    }

    #[inline]
    fn handle_edit_auto_power_off(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let mut power_off_input = crate::view::named_input::NamedInput::new(
            "Auto Power Off (days, 0 = never)".to_string(),
            ViewId::AutoPowerOffInput,
            ViewId::AutoPowerOffInput,
            10,
            context,
        );
        let text = if context.settings.auto_power_off == 0.0 {
            "0".to_string()
        } else {
            format!("{:.1}", context.settings.auto_power_off)
        };

        power_off_input.set_text(&text, rq, context);

        self.children.push(Box::new(power_off_input));
        hub.send(Event::Focus(Some(ViewId::AutoPowerOffInput))).ok();
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        true
    }

    #[inline]
    fn handle_set_button_scheme(
        &mut self,
        button_scheme: &crate::settings::ButtonScheme,
        _evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        context.settings.button_scheme = *button_scheme;
        bus.push_back(Event::Select(EntryId::SetButtonScheme(*button_scheme)));
        self.refresh_setting_values(context, rq);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
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

    #[inline]
    fn handle_set_intermission(
        &mut self,
        kind: &crate::settings::IntermKind,
        display: &crate::settings::IntermissionDisplay,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        context.settings.intermissions[*kind] = display.clone();
        self.refresh_setting_values(context, rq);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    fn handle_edit_intermission_image(
        &mut self,
        kind: &crate::settings::IntermKind,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        self.handle_close_view_event(&ViewId::SettingsValueMenu, rq);

        self.active_intermission_edit = Some(*kind);

        let initial_path = PathBuf::from("/mnt/onboard");
        let file_chooser = FileChooser::new(
            rect!(
                0,
                0,
                context.display.dims.0 as i32,
                context.display.dims.1 as i32
            ),
            initial_path,
            SelectionMode::File,
            hub,
            rq,
            context,
        );

        self.children.push(Box::new(file_chooser));
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

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
            mode: LibraryMode::Filesystem,
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

            self.rebuild_library_rows(rq, context, None);
        }

        false
    }

    #[inline]
    fn handle_submit_auto_suspend(
        &mut self,
        text: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Ok(value) = text.parse::<f32>() {
            context.settings.auto_suspend = value;
        }
        self.refresh_setting_values(context, rq);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        hub.send(Event::Focus(None)).ok();

        true
    }

    #[inline]
    fn handle_submit_auto_power_off(
        &mut self,
        text: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Ok(value) = text.parse::<f32>() {
            context.settings.auto_power_off = value;
        }

        self.refresh_setting_values(context, rq);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        hub.send(Event::Focus(None)).ok();

        true
    }

    #[inline]
    fn handle_edit_settings_retention(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let mut retention_input = crate::view::named_input::NamedInput::new(
            "Settings Retention".to_string(),
            ViewId::SettingsRetentionInput,
            ViewId::SettingsRetentionInput,
            3,
            context,
        );
        let text = context.settings.settings_retention.to_string();

        retention_input.set_text(&text, rq, context);

        self.children.push(Box::new(retention_input));
        hub.send(Event::Focus(Some(ViewId::SettingsRetentionInput)))
            .ok();

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        true
    }

    #[inline]
    fn handle_submit_settings_retention(
        &mut self,
        text: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Ok(value) = text.parse::<usize>() {
            context.settings.settings_retention = value;
        }

        self.refresh_setting_values(context, rq);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        hub.send(Event::Focus(None)).ok();

        true
    }

    /// Handles the `FileChooserClosed` event for intermission image selection.
    ///
    /// Updates `context.settings.intermissions` with the selected image path and schedules
    /// a GUI refresh to reflect the change.
    ///
    /// # Returns
    ///
    /// Always returns `false` to allow the event to propagate through the view hierarchy.
    /// Other views in the chain (LibraryEditor, SettingValue) may also need to handle this
    /// event for their own path selection needs.
    #[inline]
    fn handle_file_chooser_closed(
        &mut self,
        path: &Option<PathBuf>,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Some(kind) = self.active_intermission_edit.take() {
            if let Some(ref selected_path) = *path {
                use crate::settings::IntermissionDisplay;
                context.settings.intermissions[kind] =
                    IntermissionDisplay::Image(selected_path.clone());

                self.refresh_setting_values(context, rq);
            }
        }

        false
    }

    /// Handles the `Close` event for various child views within the category editor.
    ///
    /// This method manages the closure of different overlay and child views:
    ///
    /// - **LibraryEditor, AutoSuspendInput, AutoPowerOffInput, SettingsValueMenu**: These overlay
    ///   views are removed from the children list and a GUI update is scheduled. The event is
    ///   considered handled.
    ///
    /// - **FileChooser**: The file chooser is removed from the children list, the active
    ///   intermission edit state is cleared, and a GUI update is scheduled.
    ///
    /// - **Other view IDs**: Return false as they are not handled by this method.
    ///
    /// # Arguments
    ///
    /// * `view_id` - The ID of the view being closed
    /// * `rq` - The render queue for scheduling UI updates
    ///
    /// # Returns
    ///
    /// `true` if the event was handled, `false` otherwise.
    #[inline]
    fn handle_close_view_event(&mut self, view_id: &ViewId, rq: &mut RenderQueue) -> bool {
        match view_id {
            ViewId::LibraryEditor
            | ViewId::AutoSuspendInput
            | ViewId::AutoPowerOffInput
            | ViewId::SettingsRetentionInput
            | ViewId::SettingsValueMenu => {
                if let Some(index) = locate_by_id(self, *view_id) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            ViewId::FileChooser => {
                if let Some(index) = locate_by_id(self, ViewId::FileChooser) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                self.active_intermission_edit = None;
                false
            }
            _ => false,
        }
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn handle_toggle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
        toggle: &ToggleEvent,
    ) -> bool {
        match toggle {
            ToggleEvent::Setting(ref setting) => match setting {
                ToggleSettings::SleepCover => {
                    self.handle_toggle_sleep_cover(evt, hub, bus, rq, context)
                }

                ToggleSettings::AutoShare => {
                    self.handle_toggle_auto_share(evt, hub, bus, rq, context)
                }
                ToggleSettings::ButtonScheme => match context.settings.button_scheme {
                    ButtonScheme::Natural => self.handle_set_button_scheme(
                        &ButtonScheme::Inverted,
                        evt,
                        hub,
                        bus,
                        rq,
                        context,
                    ),
                    ButtonScheme::Inverted => self.handle_set_button_scheme(
                        &ButtonScheme::Natural,
                        evt,
                        hub,
                        bus,
                        rq,
                        context,
                    ),
                },
            },
            _ => unreachable!("mismatched toggle event"),
        }
    }
}

impl View for CategoryEditor {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::Focus(view_id) => self.handle_focus_event(view_id, hub, rq, context),
            Event::Gesture(GestureEvent::HoldFingerShort(point, _)) => {
                self.handle_hold_finger_short(point, bus, context)
            }
            Event::SubMenu(rect, ref entries) => {
                self.handle_submenu_event(rect, entries, rq, context)
            }
            Event::NewToggle(ref toggle) if matches!(toggle, ToggleEvent::Setting(_)) => {
                self.handle_toggle_event(evt, hub, bus, rq, context, toggle)
            }
            Event::Select(ref id) => match id {
                EntryId::SetKeyboardLayout(ref layout) => {
                    self.handle_set_keyboard_layout(layout, evt, hub, bus, rq, context)
                }
                EntryId::EditAutoSuspend => self.handle_edit_auto_suspend(hub, rq, context),
                EntryId::EditAutoPowerOff => self.handle_edit_auto_power_off(hub, rq, context),
                EntryId::EditSettingsRetention => {
                    self.handle_edit_settings_retention(hub, rq, context)
                }
                EntryId::SetButtonScheme(button_scheme) => {
                    self.handle_set_button_scheme(button_scheme, evt, hub, bus, rq, context)
                }
                EntryId::DeleteLibrary(index) => self.handle_delete_library(*index, rq, context),
                EntryId::SetIntermission(kind, display) => {
                    self.handle_set_intermission(kind, display, rq, context)
                }
                EntryId::EditIntermissionImage(kind) => {
                    self.handle_edit_intermission_image(kind, hub, rq, context)
                }
                _ => false,
            },
            Event::AddLibrary => self.handle_add_library_event(hub, rq, context),
            Event::EditLibrary(index) => self.handle_edit_library_event(*index, hub, rq, context),
            Event::UpdateLibrary(index, ref library) => {
                self.handle_update_library_event(*index, library, rq, context)
            }
            Event::Submit(ViewId::AutoSuspendInput, ref text) => {
                self.handle_submit_auto_suspend(text, hub, rq, context)
            }
            Event::Submit(ViewId::AutoPowerOffInput, ref text) => {
                self.handle_submit_auto_power_off(text, hub, rq, context)
            }
            Event::Submit(ViewId::SettingsRetentionInput, ref text) => {
                self.handle_submit_settings_retention(text, hub, rq, context)
            }
            Event::FileChooserClosed(ref path) => {
                self.handle_file_chooser_closed(path, rq, context)
            }
            Event::Close(view_id) => self.handle_close_view_event(view_id, rq),
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
    use crate::settings::{LibraryMode, Settings};
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    fn create_test_settings_with_libraries(count: usize) -> Settings {
        let mut settings = Settings::default();
        settings.libraries.clear();
        for i in 0..count {
            settings.libraries.push(LibrarySettings {
                name: format!("Library {}", i),
                path: PathBuf::from(format!("/mnt/onboard/lib{}", i)),
                mode: LibraryMode::Filesystem,
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
            mode: LibraryMode::Database,
            ..Default::default()
        };

        let handled = editor.handle_event(
            &Event::UpdateLibrary(0, Box::new(updated_library.clone())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(!handled);
        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Updated Library");
        assert_eq!(
            context.settings.libraries[0].path,
            PathBuf::from("/mnt/onboard/updated")
        );
        assert_eq!(context.settings.libraries[0].mode, LibraryMode::Database);
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

    #[test]
    fn test_hold_finger_shows_delete_menu() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_children_count = editor.children.len();

        let row_y = editor.content_rect.min.y + (editor.row_height / 2);
        let point = Point::new(editor.content_rect.min.x + 10, row_y);

        let handled = editor.handle_event(
            &Event::Gesture(GestureEvent::HoldFingerShort(point, 0)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(bus.len(), 1);

        if let Some(Event::SubMenu(rect, entries)) = bus.pop_front() {
            assert_eq!(entries.len(), 1);
            match &entries[0] {
                EntryKind::Command(label, entry_id) => {
                    assert_eq!(label, "Delete");
                    assert_eq!(*entry_id, EntryId::DeleteLibrary(0));
                }
                _ => panic!("Expected Command entry"),
            }

            editor.handle_event(
                &Event::SubMenu(rect, entries),
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            );

            assert_eq!(editor.children.len(), initial_children_count + 1);
            assert!(!rq.is_empty());
        } else {
            panic!("Expected SubMenu event in bus");
        }
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

        let handled = editor.handle_event(
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

        let handled = editor.handle_event(
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

    #[test]
    fn test_edit_intermission_image_opens_file_chooser() {
        use crate::settings::IntermKind;

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_children_count = editor.children.len();

        let handled = editor.handle_event(
            &Event::Select(EntryId::EditIntermissionImage(IntermKind::Share)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(editor.children.len(), initial_children_count + 1);
        assert!(editor.active_intermission_edit.is_some());
        assert_eq!(editor.active_intermission_edit.unwrap(), IntermKind::Share);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_file_chooser_closed_sets_custom_image() {
        use crate::settings::{IntermKind, IntermissionDisplay};

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        editor.active_intermission_edit = Some(IntermKind::Suspend);

        let test_path = PathBuf::from("/mnt/onboard/test.png");
        let handled = editor.handle_event(
            &Event::FileChooserClosed(Some(test_path.clone())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(!handled);
        assert!(editor.active_intermission_edit.is_none());
        assert!(matches!(
            &context.settings.intermissions[IntermKind::Suspend],
            IntermissionDisplay::Image(path) if path == &test_path
        ));
    }

    #[test]
    fn test_file_chooser_cancelled_clears_active_edit() {
        use crate::settings::IntermKind;

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        editor.active_intermission_edit = Some(IntermKind::Share);

        let handled = editor.handle_event(
            &Event::FileChooserClosed(None),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(!handled);
        assert!(editor.active_intermission_edit.is_none());
    }

    #[test]
    fn test_close_file_chooser_clears_active_edit() {
        use crate::settings::IntermKind;

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        editor.active_intermission_edit = Some(IntermKind::PowerOff);
        editor
            .children
            .push(Box::new(crate::view::filler::Filler::new(
                rect![0, 0, 100, 100],
                crate::color::WHITE,
            )));

        let handled = editor.handle_event(
            &Event::Close(ViewId::FileChooser),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(
            !handled,
            "Close event for FileChooser should not capture the event so that settings editor can refresh the whole screen.");
        assert!(editor.active_intermission_edit.is_none());
    }
}
