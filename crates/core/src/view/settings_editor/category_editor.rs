use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, Rectangle};
use crate::gesture::GestureEvent;
use crate::settings::{LibraryMode, LibrarySettings, Settings};
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::filler::Filler;
use crate::view::menu::Menu;
use crate::view::toggleable_keyboard::ToggleableKeyboard;
use crate::view::top_bar::{TopBar, TopBarVariant};
use crate::view::{
    Bus, EntryId, EntryKind, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, BIG_BAR_HEIGHT,
    ID_FEEDER, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM,
};
use anyhow::Error;
use std::path::PathBuf;

use super::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};
use super::category::Category;
use super::library_editor::LibraryEditor;
use super::setting_row::{Kind as RowKind, SettingRow};

/// A view for editing category-specific settings.
///
/// The `CategoryEditor` manages the UI for editing settings within a specific category
/// (e.g., Libraries, Intermissions, etc.). It displays setting rows, handles user interactions,
/// and manages child views such as keyboards, input fields, and file choosers.
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - The rectangular area occupied by this view
/// * `children` - Child views managed by this editor (rows, keyboard, menus, etc.)
/// * `category` - The settings category being edited
/// * `settings` - Current settings being edited
/// * `original_settings` - Original settings before any edits (used for cancellation)
/// * `content_rect` - The rectangular area where setting rows are displayed
/// * `row_height` - The height of each setting row
/// * `focus` - Currently focused child view, if any
/// * `keyboard_index` - Index of the keyboard child view in the children vector
/// * `active_intermission_edit` - Tracks which intermission type is currently being edited via file chooser
pub struct CategoryEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    category: Category,
    settings: Settings,
    original_settings: Settings,
    content_rect: Rectangle,
    row_height: i32,
    focus: Option<ViewId>,
    keyboard_index: usize,
    active_intermission_edit: Option<crate::settings::IntermKind>,
}

impl CategoryEditor {
    pub fn new(
        rect: Rectangle,
        category: Category,
        _hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> Result<CategoryEditor, Error> {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();
        let settings = context.settings.clone();

        let (bar_height, _separator_thickness, separator_top_half, separator_bottom_half) =
            Self::calculate_dimensions();

        children.push(Self::build_top_bar(
            rect,
            bar_height,
            separator_top_half,
            category,
            context,
        ));

        children.push(Self::build_top_separator(
            rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        let (background, content_rect) = Self::build_content_background(
            rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        );
        children.push(background);

        let row_height = scale_by_dpi(BIG_BAR_HEIGHT, CURRENT_DEVICE.dpi) as i32;
        let setting_kinds = category.settings(context);
        let mut current_y = content_rect.min.y;

        for kind in setting_kinds {
            let row_rect = rect![
                content_rect.min.x,
                current_y,
                content_rect.max.x,
                current_y + row_height
            ];
            children.push(Self::build_setting_row(kind, row_rect, &settings)?);
            current_y += row_height;
        }

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

        let keyboard = ToggleableKeyboard::new(rect, true);
        children.push(Box::new(keyboard) as Box<dyn View>);

        let keyboard_index = children.len() - 1;

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        Ok(CategoryEditor {
            id,
            rect,
            children,
            category,
            original_settings: settings.clone(),
            settings,
            content_rect,
            row_height,
            focus: None,
            keyboard_index,
            active_intermission_edit: None,
        })
    }

    fn calculate_dimensions() -> (i32, i32, i32, i32) {
        let dpi = CURRENT_DEVICE.dpi;
        let (small_height, _big_height) = (
            scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32,
            scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32,
        );
        let separator_thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (separator_top_half, separator_bottom_half) = halves(separator_thickness);
        let bar_height = small_height;

        (
            bar_height,
            separator_thickness,
            separator_top_half,
            separator_bottom_half,
        )
    }

    fn build_top_bar(
        rect: Rectangle,
        bar_height: i32,
        separator_top_half: i32,
        category: Category,
        context: &mut Context,
    ) -> Box<dyn View> {
        let top_bar = TopBar::new(
            rect![
                rect.min.x,
                rect.min.y,
                rect.max.x,
                rect.min.y + bar_height - separator_top_half
            ],
            TopBarVariant::Cancel(Event::Close(ViewId::SettingsCategoryEditor)),
            category.label(),
            context,
        );
        Box::new(top_bar) as Box<dyn View>
    }

    fn build_top_separator(
        rect: Rectangle,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        let separator = Filler::new(
            rect![
                rect.min.x,
                rect.min.y + bar_height - separator_top_half,
                rect.max.x,
                rect.min.y + bar_height + separator_bottom_half
            ],
            BLACK,
        );
        Box::new(separator) as Box<dyn View>
    }

    fn build_content_background(
        rect: Rectangle,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> (Box<dyn View>, Rectangle) {
        let content_rect = rect![
            rect.min.x,
            rect.min.y + bar_height + separator_bottom_half,
            rect.max.x,
            rect.max.y - bar_height - separator_top_half
        ];

        let background = Filler::new(content_rect, WHITE);
        (Box::new(background) as Box<dyn View>, content_rect)
    }

    fn build_setting_row(
        kind: RowKind,
        row_rect: Rectangle,
        settings: &Settings,
    ) -> Result<Box<dyn View>, Error> {
        let setting_row = SettingRow::new(kind, row_rect, settings);
        Ok(Box::new(setting_row) as Box<dyn View>)
    }

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
                BottomBarVariant::TwoButtons {
                    left_event: Event::AddLibrary,
                    left_icon: "plus",
                    right_event: Event::Validate,
                    right_icon: "check_mark-large",
                },
            )),
            _ => Box::new(SettingsEditorBottomBar::new(
                bottom_bar_rect,
                BottomBarVariant::SingleButton {
                    event: Event::Validate,
                    icon: "check_mark-large",
                },
            )),
        }
    }

    /// Rebuilds the library rows in the UI after a library is added, removed, or modified.
    ///
    /// This method removes the old library rows and inserts new ones based on the current
    /// state of `self.settings.libraries`. It only operates when the current category is
    /// `Category::Libraries`.
    ///
    /// # Arguments
    ///
    /// * `rq` - The render queue to add render updates to
    /// * `original_count` - The original number of library rows before the change. If `None`,
    ///   uses the current library count. This is used to determine how many old rows to remove.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the operation succeeds, or an error if the operation fails.
    /// If the current category is not `Category::Libraries`, returns `Ok(())` immediately.
    fn rebuild_library_rows(
        &mut self,
        rq: &mut RenderQueue,
        original_count: Option<usize>,
    ) -> Result<(), Error> {
        if self.category != Category::Libraries {
            return Ok(());
        }

        let num_libraries = self.settings.libraries.len();
        let rows_to_remove = original_count.unwrap_or(num_libraries);

        let first_row_index = 3;

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

            let setting_row = SettingRow::new(RowKind::Library(i), row_rect, &self.settings);

            new_rows.push(Box::new(setting_row) as Box<dyn View>);
            current_y += self.row_height;
        }

        for (offset, row) in new_rows.into_iter().enumerate() {
            self.children.insert(first_row_index + offset, row);
        }

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        Ok(())
    }

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

    /// Updates all child setting views to reflect the current settings in the UI.
    ///
    /// This method propagates the given event to all grandchildren (nested views) of this editor,
    /// ensuring that any setting value displays are refreshed to show the latest state of the settings.
    /// This is typically called after a setting has been modified to ensure the UI remains in sync
    /// with the underlying settings data.
    ///
    /// # Arguments
    ///
    /// * `evt` - The event to propagate to child views
    /// * `hub` - The event hub for sending messages
    /// * `bus` - The event bus for queueing events
    /// * `rq` - The render queue for scheduling UI updates
    /// * `context` - The application context containing settings and device information
    fn update_setting_values(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        for child in &mut self.children {
            for grandchild in child.children_mut() {
                grandchild.handle_event(evt, hub, bus, rq, context);
            }
        }
    }

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
    fn handle_hold_finger_short(&mut self, point: &crate::geom::Point, bus: &mut Bus) -> bool {
        if self.category != Category::Libraries {
            return false;
        }

        if !self.content_rect.includes(*point) {
            return false;
        }

        let row_index = (point.y - self.content_rect.min.y) / self.row_height;
        let library_index = row_index as usize;

        if library_index < self.settings.libraries.len() {
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
            crate::view::menu::MenuKind::SubMenu,
            entries.to_vec(),
            context,
        );

        rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
        self.children.push(Box::new(menu));

        true
    }

    fn handle_set_keyboard_layout(
        &mut self,
        layout: &str,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        self.settings.keyboard_layout = layout.to_string();
        self.update_setting_values(evt, hub, bus, rq, context);
        true
    }

    fn handle_toggle_sleep_cover(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        self.settings.sleep_cover = !self.settings.sleep_cover;
        self.update_setting_values(evt, hub, bus, rq, context);
        true
    }

    fn handle_toggle_auto_share(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        self.settings.auto_share = !self.settings.auto_share;
        self.update_setting_values(evt, hub, bus, rq, context);
        true
    }

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
        let text = if self.settings.auto_suspend == 0.0 {
            "0".to_string()
        } else {
            format!("{:.1}", self.settings.auto_suspend)
        };

        suspend_input.set_text(&text, rq, context);

        self.children.push(Box::new(suspend_input));
        hub.send(Event::Focus(Some(ViewId::AutoSuspendInput))).ok();

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        true
    }

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
        let text = if self.settings.auto_power_off == 0.0 {
            "0".to_string()
        } else {
            format!("{:.1}", self.settings.auto_power_off)
        };

        power_off_input.set_text(&text, rq, context);

        self.children.push(Box::new(power_off_input));
        hub.send(Event::Focus(Some(ViewId::AutoPowerOffInput))).ok();
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        true
    }

    fn handle_set_button_scheme(
        &mut self,
        button_scheme: &crate::settings::ButtonScheme,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        self.settings.button_scheme = *button_scheme;
        self.update_setting_values(evt, hub, bus, rq, context);
        true
    }

    fn handle_delete_library(&mut self, index: usize, rq: &mut RenderQueue) -> bool {
        if index < self.settings.libraries.len() {
            let original_count = self.settings.libraries.len();
            self.settings.libraries.remove(index);

            if let Err(e) = self.rebuild_library_rows(rq, Some(original_count)) {
                eprintln!("Failed to rebuild library rows: {}", e);
            }
        }

        if let Some(menu_index) = locate_by_id(self, ViewId::SettingsValueMenu) {
            self.children.remove(menu_index);
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }

        true
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_set_intermission(
        &mut self,
        kind: &crate::settings::IntermKind,
        display: &crate::settings::IntermissionDisplay,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        self.settings.intermissions[*kind] = display.clone();
        self.update_setting_values(evt, hub, bus, rq, context);
        true
    }

    fn handle_edit_intermission_image(
        &mut self,
        kind: &crate::settings::IntermKind,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        use crate::view::file_chooser::{FileChooser, SelectionMode};

        self.active_intermission_edit = Some(*kind);

        let initial_path = PathBuf::from("/mnt/onboard");
        let file_chooser = FileChooser::new(
            self.rect,
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

    fn handle_validate_event(&mut self, bus: &mut Bus) -> bool {
        bus.push_back(Event::UpdateSettings(Box::new(self.settings.clone())));
        bus.push_back(Event::Close(ViewId::SettingsCategoryEditor));
        true
    }

    fn handle_add_library_event(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let library = LibrarySettings {
            name: String::new(),
            path: PathBuf::from("/mnt/onboard"),
            mode: LibraryMode::Filesystem,
            ..Default::default()
        };

        let library_index = self.settings.libraries.len();
        self.settings.libraries.push(library.clone());

        if let Err(e) = self.rebuild_library_rows(rq, None) {
            eprintln!("Failed to rebuild library rows: {}", e);
        }

        if let Ok(library_editor) =
            LibraryEditor::new(self.rect, library_index, library, hub, rq, context)
        {
            self.children.push(Box::new(library_editor));
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }

        true
    }

    fn handle_edit_library_event(
        &mut self,
        index: usize,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Some(library) = self.settings.libraries.get(index).cloned() {
            if let Ok(library_editor) =
                LibraryEditor::new(self.rect, index, library, hub, rq, context)
            {
                self.children.push(Box::new(library_editor));
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
            }
        }
        true
    }

    fn handle_update_library_event(
        &mut self,
        index: usize,
        library: &LibrarySettings,
        rq: &mut RenderQueue,
    ) -> bool {
        if index < self.settings.libraries.len() {
            self.settings.libraries[index] = library.clone();

            if let Err(e) = self.rebuild_library_rows(rq, None) {
                eprintln!("Failed to rebuild library rows: {}", e);
            }
        }

        false
    }

    fn handle_submit_auto_suspend(
        &mut self,
        text: &str,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Ok(value) = text.parse::<f32>() {
            self.settings.auto_suspend = value;
        }
        self.update_setting_values(evt, hub, bus, rq, context);

        hub.send(Event::Focus(None)).ok();

        true
    }

    fn handle_submit_auto_power_off(
        &mut self,
        text: &str,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Ok(value) = text.parse::<f32>() {
            self.settings.auto_power_off = value;
        }

        self.update_setting_values(evt, hub, bus, rq, context);

        hub.send(Event::Focus(None)).ok();

        true
    }

    fn handle_file_chooser_closed(
        &mut self,
        path: &Option<PathBuf>,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Some(kind) = self.active_intermission_edit.take() {
            if let Some(ref selected_path) = *path {
                use crate::settings::IntermissionDisplay;
                self.settings.intermissions[kind] =
                    IntermissionDisplay::Image(selected_path.clone());

                let display_name = selected_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Custom")
                    .to_string();

                let update_event = match kind {
                    crate::settings::IntermKind::Suspend => {
                        Event::Submit(ViewId::IntermissionSuspendInput, display_name)
                    }
                    crate::settings::IntermKind::PowerOff => {
                        Event::Submit(ViewId::IntermissionPowerOffInput, display_name)
                    }
                    crate::settings::IntermKind::Share => {
                        Event::Submit(ViewId::IntermissionShareInput, display_name)
                    }
                };

                self.update_setting_values(&update_event, hub, bus, rq, context);
            }
        }

        false
    }

    fn handle_close_view_event(&mut self, view_id: &ViewId, rq: &mut RenderQueue) -> bool {
        match view_id {
            ViewId::SettingsCategoryEditor => {
                self.settings = self.original_settings.clone();
                false
            }
            ViewId::LibraryEditor
            | ViewId::AutoSuspendInput
            | ViewId::AutoPowerOffInput
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
                true
            }
            _ => false,
        }
    }
}

impl View for CategoryEditor {
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
            Event::Back => {
                bus.push_back(Event::Close(ViewId::SettingsCategoryEditor));
                true
            }
            Event::Gesture(GestureEvent::HoldFingerShort(point, _)) => {
                self.handle_hold_finger_short(point, bus)
            }
            Event::SubMenu(rect, ref entries) => {
                self.handle_submenu_event(rect, entries, rq, context)
            }
            Event::Select(ref id) => match id {
                EntryId::SetKeyboardLayout(ref layout) => {
                    self.handle_set_keyboard_layout(layout, evt, hub, bus, rq, context)
                }
                EntryId::ToggleSleepCover => {
                    self.handle_toggle_sleep_cover(evt, hub, bus, rq, context)
                }
                EntryId::ToggleAutoShare => {
                    self.handle_toggle_auto_share(evt, hub, bus, rq, context)
                }
                EntryId::EditAutoSuspend => self.handle_edit_auto_suspend(hub, rq, context),
                EntryId::EditAutoPowerOff => self.handle_edit_auto_power_off(hub, rq, context),
                EntryId::SetButtonScheme(button_scheme) => {
                    self.handle_set_button_scheme(button_scheme, evt, hub, bus, rq, context)
                }
                EntryId::DeleteLibrary(index) => self.handle_delete_library(*index, rq),
                EntryId::SetIntermission(kind, display) => {
                    self.handle_set_intermission(kind, display, evt, hub, bus, rq, context)
                }
                EntryId::EditIntermissionImage(kind) => {
                    self.handle_edit_intermission_image(kind, hub, rq, context)
                }
                _ => false,
            },
            Event::Validate => self.handle_validate_event(bus),
            Event::AddLibrary => self.handle_add_library_event(hub, rq, context),
            Event::EditLibrary(index) => self.handle_edit_library_event(*index, hub, rq, context),
            Event::UpdateLibrary(index, ref library) => {
                self.handle_update_library_event(*index, library, rq)
            }
            Event::Submit(ViewId::AutoSuspendInput, ref text) => {
                self.handle_submit_auto_suspend(text, evt, hub, bus, rq, context)
            }
            Event::Submit(ViewId::AutoPowerOffInput, ref text) => {
                self.handle_submit_auto_power_off(text, evt, hub, bus, rq, context)
            }
            Event::FileChooserClosed(ref path) => {
                self.handle_file_chooser_closed(path, hub, bus, rq, context)
            }
            Event::Close(view_id) => self.handle_close_view_event(view_id, rq),
            _ => false,
        }
    }

    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut crate::font::Fonts) {
    }

    fn is_background(&self) -> bool {
        true
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

    fn view_id(&self) -> Option<ViewId> {
        Some(ViewId::SettingsCategoryEditor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::{Battery, FakeBattery};
    use crate::font::Fonts;
    use crate::framebuffer::Pixmap;
    use crate::frontlight::{Frontlight, LightLevels};
    use crate::geom::Point;
    use crate::library::Library;
    use crate::lightsensor::LightSensor;
    use crate::settings::{LibraryMode, Settings};
    use std::collections::VecDeque;
    use std::env;
    use std::path::Path;
    use std::sync::mpsc::channel;

    fn create_test_context() -> Context {
        let fb = Box::new(Pixmap::new(600, 800, 1)) as Box<dyn Framebuffer>;
        let battery = Box::new(FakeBattery::new()) as Box<dyn Battery>;
        let frontlight = Box::new(LightLevels::default()) as Box<dyn Frontlight>;
        let lightsensor = Box::new(0u16) as Box<dyn LightSensor>;
        let settings = Settings::default();
        let library = Library::new(Path::new("."), LibraryMode::Database).unwrap_or_else(|_| {
            Library::new(Path::new("/tmp"), LibraryMode::Database).expect(
                "Failed to create test library. \
                 Ensure /tmp directory exists and is writable.",
            )
        });
        let fonts = Fonts::load_from(
            Path::new(
                &env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for this test."),
            )
            .to_path_buf(),
        )
        .expect(
            "Failed to load fonts. Tests require font files to be present. \
             Run tests from the project root directory.",
        );

        let mut ctx = Context::new(
            fb,
            None,
            library,
            settings,
            fonts,
            battery,
            frontlight,
            lightsensor,
        );
        ctx.load_keyboard_layouts();
        ctx.load_dictionaries();

        ctx
    }

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

    fn create_test_category_editor_with_context(
        context: &mut Context,
    ) -> Result<CategoryEditor, Error> {
        let rect = rect![0, 0, 600, 800];
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();

        CategoryEditor::new(rect, Category::Libraries, &hub, &mut rq, context)
    }

    #[test]
    fn test_add_library_event() {
        let mut context = create_test_context();
        context.settings = Settings::default();
        context.settings.libraries.clear();
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(editor.settings.libraries.len(), 0);
        let initial_children_count = editor.children.len();

        let handled =
            editor.handle_event(&Event::AddLibrary, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(editor.settings.libraries.len(), 1);

        let added_library = &editor.settings.libraries[0];
        assert_eq!(added_library.name, String::new());
        assert_eq!(added_library.path, PathBuf::from("/mnt/onboard"));
        assert_eq!(added_library.mode, LibraryMode::Filesystem);

        assert_eq!(editor.children.len(), initial_children_count + 1);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_delete_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(2);
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(editor.settings.libraries.len(), 2);
        assert_eq!(editor.settings.libraries[0].name, "Library 0");
        assert_eq!(editor.settings.libraries[1].name, "Library 1");

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
        assert_eq!(editor.settings.libraries.len(), 1);
        assert_eq!(editor.settings.libraries[0].name, "Library 1");

        assert!(!rq.is_empty());
    }

    #[test]
    fn test_update_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(editor.settings.libraries.len(), 1);
        assert_eq!(editor.settings.libraries[0].name, "Library 0");

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
        assert_eq!(editor.settings.libraries.len(), 1);
        assert_eq!(editor.settings.libraries[0].name, "Updated Library");
        assert_eq!(
            editor.settings.libraries[0].path,
            PathBuf::from("/mnt/onboard/updated")
        );
        assert_eq!(editor.settings.libraries[0].mode, LibraryMode::Database);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_edit_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
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
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
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

    fn create_test_intermissions_category_editor(
        context: &mut Context,
    ) -> Result<CategoryEditor, Error> {
        let rect = rect![0, 0, 600, 800];
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();

        CategoryEditor::new(rect, Category::Intermissions, &hub, &mut rq, context)
    }

    #[test]
    fn test_set_intermission_logo() {
        use crate::settings::{IntermKind, IntermissionDisplay};

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context)
            .expect("Failed to create intermissions category editor");
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
            editor.settings.intermissions[IntermKind::Suspend],
            IntermissionDisplay::Logo
        ));
    }

    #[test]
    fn test_set_intermission_cover() {
        use crate::settings::{IntermKind, IntermissionDisplay};

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context)
            .expect("Failed to create intermissions category editor");
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
            editor.settings.intermissions[IntermKind::PowerOff],
            IntermissionDisplay::Cover
        ));
    }

    #[test]
    fn test_edit_intermission_image_opens_file_chooser() {
        use crate::settings::IntermKind;

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context)
            .expect("Failed to create intermissions category editor");
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
        let mut editor = create_test_intermissions_category_editor(&mut context)
            .expect("Failed to create intermissions category editor");
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
            &editor.settings.intermissions[IntermKind::Suspend],
            IntermissionDisplay::Image(path) if path == &test_path
        ));
    }

    #[test]
    fn test_file_chooser_cancelled_clears_active_edit() {
        use crate::settings::IntermKind;

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context)
            .expect("Failed to create intermissions category editor");
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
        let mut editor = create_test_intermissions_category_editor(&mut context)
            .expect("Failed to create intermissions category editor");
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

        assert!(handled);
        assert!(editor.active_intermission_edit.is_none());
    }
}
