use super::bottom_bar::BottomBarVariant;
use super::editor_utils::{
    build_bottom_separator, build_two_button_bottom_bar, calculate_dimensions,
};
use super::kinds::SettingIdentity;
use super::kinds::library::{LibraryFinishedAction, LibraryName, LibraryPath};
use super::setting_row::SettingRow;
use super::setting_value::SettingsEvent;
use crate::color::WHITE;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::fl;
use crate::font::Fonts;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::settings::{FinishedAction, LibrarySettings, Settings};
use crate::unit::scale_by_dpi;
use crate::view::SMALL_BAR_HEIGHT;
use crate::view::common::locate_by_id;
use crate::view::file_chooser::{FileChooser, SelectionMode};
use crate::view::filler::Filler;
use crate::view::menu::{Menu, MenuKind};
use crate::view::named_input::NamedInput;
use crate::view::toggleable_keyboard::ToggleableKeyboard;
use crate::view::{Bus, Event, Hub, ID_FEEDER, Id, RenderData, RenderQueue, View, ViewId};
use crate::view::{EntryId, NotificationEvent};

/// A view for editing library settings.
///
/// The `LibraryEditor` provides a user interface for configuring library properties
/// such as name, path, and mode. It manages a collection of child views including
/// setting rows, a keyboard for text input, and various overlays (dialogs, menus).
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - The rectangular area occupied by this editor
/// * `children` - Child views including separators, rows, bars, and overlays
/// * `library_index` - Index of the library being edited in the settings
/// * `library` - Current library settings being edited
/// * `_original_library` - Original library settings before modifications (for potential rollback)
/// * `focus` - The currently focused child view, if any
/// * `keyboard_index` - Index of the keyboard view in the children vector
pub struct LibraryEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    library_index: usize,
    library: LibrarySettings,
    _original_library: LibrarySettings,
    focus: Option<ViewId>,
    keyboard_index: usize,
}

impl LibraryEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(_hub, context, rq)))]
    pub fn new(
        rect: Rectangle,
        library_index: usize,
        library: LibrarySettings,
        _hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> LibraryEditor {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        let mut settings = context.settings.clone();
        if library_index <= settings.libraries.len() {
            settings.libraries.insert(library_index, library.clone());
        }
        let settings = settings;

        children.push(Box::new(Filler::new(rect, WHITE)) as Box<dyn View>);

        let (bar_height, separator_thickness, separator_top_half, separator_bottom_half) =
            calculate_dimensions();

        children.extend(Self::build_content_rows(
            rect,
            bar_height,
            separator_thickness,
            library_index,
            &settings,
            &mut context.fonts,
        ));

        children.push(build_bottom_separator(
            rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));
        children.push(Self::build_bottom_bar(
            rect,
            bar_height,
            separator_bottom_half,
        ));

        let keyboard = ToggleableKeyboard::new(rect, false);
        children.push(Box::new(keyboard) as Box<dyn View>);

        let keyboard_index = children.len() - 1;

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        LibraryEditor {
            id,
            rect,
            children,
            library_index,
            library: library.clone(),
            _original_library: library,
            focus: None,
            keyboard_index,
        }
    }

    #[inline]
    fn build_content_rows(
        rect: Rectangle,
        bar_height: i32,
        separator_thickness: i32,
        library_index: usize,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> Vec<Box<dyn View>> {
        let mut children = Vec::new();
        let dpi = CURRENT_DEVICE.dpi;
        let row_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;

        let content_start_y = rect.min.y;
        let content_end_y = rect.max.y - bar_height - separator_thickness;

        let mut current_y = content_start_y;

        if current_y + row_height <= content_end_y {
            let name_row_rect = rect![rect.min.x, current_y, rect.max.x, current_y + row_height];
            children.push(Self::build_name_row(
                name_row_rect,
                library_index,
                settings,
                fonts,
            ));
            current_y += row_height;
        }

        if current_y + row_height <= content_end_y {
            let path_row_rect = rect![rect.min.x, current_y, rect.max.x, current_y + row_height];
            children.push(Self::build_path_row(
                path_row_rect,
                library_index,
                settings,
                fonts,
            ));
            current_y += row_height;
        }

        if current_y + row_height <= content_end_y {
            let finished_row_rect =
                rect![rect.min.x, current_y, rect.max.x, current_y + row_height];
            children.push(Self::build_finished_action_row(
                finished_row_rect,
                library_index,
                settings,
                fonts,
            ));
        }

        children
    }

    #[inline]
    fn build_name_row(
        rect: Rectangle,
        library_index: usize,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> Box<dyn View> {
        Box::new(SettingRow::new(
            Box::new(LibraryName(library_index)),
            rect,
            settings,
            fonts,
        )) as Box<dyn View>
    }

    fn build_path_row(
        rect: Rectangle,
        library_index: usize,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> Box<dyn View> {
        Box::new(SettingRow::new(
            Box::new(LibraryPath(library_index)),
            rect,
            settings,
            fonts,
        )) as Box<dyn View>
    }

    #[inline]
    fn build_finished_action_row(
        rect: Rectangle,
        library_index: usize,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> Box<dyn View> {
        Box::new(SettingRow::new(
            Box::new(LibraryFinishedAction(library_index)),
            rect,
            settings,
            fonts,
        )) as Box<dyn View>
    }

    #[inline]
    fn build_bottom_bar(
        rect: Rectangle,
        bar_height: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        build_two_button_bottom_bar(
            rect,
            bar_height,
            separator_bottom_half,
            BottomBarVariant::TwoButtons {
                left_event: Event::Close(ViewId::LibraryEditor),
                left_icon: "close",
                right_event: Event::Validate,
                right_icon: "check_mark-large",
            },
        )
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
        focus: Option<ViewId>,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if self.focus != focus {
            self.focus = focus;
            if focus.is_some() {
                self.toggle_keyboard(true, focus, hub, rq, context);
            } else {
                self.toggle_keyboard(false, None, hub, rq, context);
            }
        }
        true
    }

    #[inline]
    fn handle_validate_event(&self, hub: &Hub, bus: &mut Bus) -> bool {
        if self.library.name.trim().is_empty() {
            hub.send(Event::Notification(NotificationEvent::Show(
                "Library name cannot be empty".to_string(),
            )))
            .ok();
            return true;
        }

        if !self.library.path.exists() {
            hub.send(Event::Notification(NotificationEvent::Show(
                "Path does not exist".to_string(),
            )))
            .ok();
            return true;
        }

        bus.push_back(Event::UpdateLibrary(
            self.library_index,
            Box::new(self.library.clone()),
        ));
        bus.push_back(Event::Close(ViewId::LibraryEditor));

        true
    }

    #[inline]
    fn handle_edit_name_event(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let mut name_input = NamedInput::new(
            "Library Name".to_string(),
            ViewId::LibraryRename,
            ViewId::LibraryRenameInput,
            10,
            context,
        );
        name_input.set_text(&self.library.name, rq, context);

        self.children.push(Box::new(name_input));
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        hub.send(Event::Focus(Some(ViewId::LibraryRenameInput)))
            .ok();
        true
    }

    #[inline]
    fn handle_edit_path_event(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let screen_rect = rect!(
            0,
            0,
            context.display.dims.0 as i32,
            context.display.dims.1 as i32
        );

        let file_chooser = FileChooser::new(
            screen_rect,
            self.library.path.clone(),
            SelectionMode::Directory,
            hub,
            rq,
            context,
        );
        self.children.push(Box::new(file_chooser));
        rq.add(RenderData::new(self.id, screen_rect, UpdateMode::Gui));

        true
    }

    #[inline]
    fn handle_submit_name_event(&mut self, text: &str, bus: &mut Bus) -> bool {
        self.library.name = text.to_string();
        bus.push_back(Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryName(self.library_index),
            value: text.to_string(),
        }));
        false
    }

    #[inline]
    fn handle_set_library_finished_action(
        &mut self,
        index: usize,
        action: FinishedAction,
        bus: &mut Bus,
    ) -> bool {
        if index != self.library_index {
            return false;
        }
        self.library.finished = Some(action);
        bus.push_back(Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryFinishedAction(self.library_index),
            value: action.to_string(),
        }));
        true
    }

    #[inline]
    fn handle_clear_library_finished_action(&mut self, index: usize, bus: &mut Bus) -> bool {
        if index != self.library_index {
            return false;
        }
        self.library.finished = None;
        bus.push_back(Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryFinishedAction(self.library_index),
            value: fl!("settings-library-inherit"),
        }));
        true
    }

    #[inline]
    fn handle_file_chooser_closed_event(
        &mut self,
        path: &Option<std::path::PathBuf>,
        bus: &mut Bus,
    ) -> bool {
        if let Some(path) = path {
            self.library.path = path.clone();
            bus.push_back(Event::Settings(SettingsEvent::UpdateValue {
                kind: SettingIdentity::LibraryPath(self.library_index),
                value: path.display().to_string(),
            }));
        }
        false
    }

    #[inline]
    fn handle_submenu_event(
        &mut self,
        rect: Rectangle,
        entries: &[crate::view::EntryKind],
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let menu = Menu::new(
            rect,
            ViewId::SettingsValueMenu,
            MenuKind::Contextual,
            entries.to_vec(),
            context,
        );
        rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
        self.children.push(Box::new(menu));
        true
    }

    /// Handles closing of overlay views (menus, dialogs, file choosers).
    ///
    /// Removes the specified view from the children vector and triggers a re-render.
    /// When closing the rename dialog, also clears focus to hide the keyboard.
    ///
    /// # Arguments
    ///
    /// * `view_id` - The ID of the view to close
    /// * `hub` - Event hub for sending focus events
    /// * `rq` - Render queue for scheduling screen updates
    ///
    /// # Returns
    ///
    /// `true` if the event was handled and should not propagate to parent,
    /// `false` if the parent should handle additional cleanup (e.g., FileChooser
    /// requires the parent to redraw the entire screen as it temporarily captures
    /// the full display area).
    #[inline]
    fn handle_close_event(&mut self, view_id: ViewId, hub: &Hub, rq: &mut RenderQueue) -> bool {
        match view_id {
            ViewId::SettingsValueMenu => {
                if let Some(index) = locate_by_id(self, ViewId::SettingsValueMenu) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            ViewId::LibraryRename => {
                if let Some(index) = locate_by_id(self, ViewId::LibraryRename) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                hub.send(Event::Focus(None)).ok();
                true
            }
            ViewId::FileChooser => {
                if let Some(index) = locate_by_id(self, ViewId::FileChooser) {
                    self.children.remove(index);
                }
                false
            }
            _ => false,
        }
    }
}

impl View for LibraryEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::HoldFingerShort(_, _)) => true,
            Event::Focus(v) => self.handle_focus_event(v, hub, rq, context),
            Event::Validate => self.handle_validate_event(hub, bus),
            Event::Select(EntryId::EditLibraryName) => {
                self.handle_edit_name_event(hub, rq, context)
            }
            Event::Select(EntryId::EditLibraryPath) => {
                self.handle_edit_path_event(hub, rq, context)
            }
            Event::Select(EntryId::SetLibraryFinishedAction(index, action)) => {
                self.handle_set_library_finished_action(index, action, bus)
            }
            Event::Select(EntryId::ClearLibraryFinishedAction(index)) => {
                self.handle_clear_library_finished_action(index, bus)
            }
            Event::Submit(ViewId::LibraryRenameInput, ref text) => {
                self.handle_submit_name_event(text, bus)
            }
            Event::FileChooserClosed(ref path) => self.handle_file_chooser_closed_event(path, bus),
            Event::SubMenu(rect, ref entries) => {
                self.handle_submenu_event(rect, entries, rq, context)
            }
            Event::Close(view) => self.handle_close_event(view, hub, rq),
            _ => false,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}

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
        Some(ViewId::LibraryEditor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    fn create_test_library() -> LibrarySettings {
        LibrarySettings {
            name: "Test Library".to_string(),
            path: std::path::PathBuf::from("/tmp"),
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_empty_name_shows_notification() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 600, 800];
        let (hub, receiver) = channel();
        let mut rq = RenderQueue::new();

        let mut library = create_test_library();
        library.name = "".to_string();

        let mut editor = LibraryEditor::new(rect, 0, library, &hub, &mut rq, &mut context);

        let mut bus = VecDeque::new();

        let handled = editor.handle_event(&Event::Validate, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(bus.len(), 0);

        if let Ok(Event::Notification(NotificationEvent::Show(msg))) = receiver.try_recv() {
            assert_eq!(msg, "Library name cannot be empty");
        } else {
            panic!("Expected notification event about empty name");
        }
    }

    #[test]
    fn test_validate_nonexistent_path_shows_notification() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 600, 800];
        let (hub, receiver) = channel();
        let mut rq = RenderQueue::new();

        let mut library = create_test_library();
        library.path = std::path::PathBuf::from("/nonexistent/path/that/does/not/exist");

        let mut editor = LibraryEditor::new(rect, 0, library, &hub, &mut rq, &mut context);

        let mut bus = VecDeque::new();

        let handled = editor.handle_event(&Event::Validate, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(bus.len(), 0);

        if let Ok(Event::Notification(NotificationEvent::Show(msg))) = receiver.try_recv() {
            assert_eq!(msg, "Path does not exist");
        } else {
            panic!("Expected notification event about nonexistent path");
        }
    }

    #[test]
    fn test_validate_success_emits_update_and_close() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 600, 800];
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();

        let library = create_test_library();
        let library_index = 0;

        let mut editor = LibraryEditor::new(
            rect,
            library_index,
            library.clone(),
            &hub,
            &mut rq,
            &mut context,
        );

        let mut bus = VecDeque::new();

        let handled = editor.handle_event(&Event::Validate, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(bus.len(), 2);

        if let Some(Event::UpdateLibrary(idx, lib)) = bus.pop_front() {
            assert_eq!(idx, library_index);
            assert_eq!(lib.name, library.name);
        } else {
            panic!("Expected UpdateLibrary event");
        }

        if let Some(Event::Close(view_id)) = bus.pop_front() {
            assert_eq!(view_id, ViewId::LibraryEditor);
        } else {
            panic!("Expected Close event");
        }
    }

    #[test]
    fn test_edit_library_name_opens_input() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 600, 800];
        let (hub, receiver) = channel();
        let mut rq = RenderQueue::new();

        let library = create_test_library();

        let mut editor = LibraryEditor::new(rect, 0, library, &hub, &mut rq, &mut context);

        let initial_children_count = editor.children.len();

        let mut bus = VecDeque::new();

        let handled = editor.handle_event(
            &Event::Select(EntryId::EditLibraryName),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(editor.children.len(), initial_children_count + 1);
        assert!(!rq.is_empty());

        if let Ok(Event::Focus(Some(ViewId::LibraryRenameInput))) = receiver.try_recv() {
        } else {
            panic!("Expected Focus event for LibraryRenameInput");
        }
    }

    #[test]
    fn test_edit_library_path_opens_file_chooser() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 600, 800];
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();

        let library = create_test_library();

        let mut editor = LibraryEditor::new(rect, 0, library, &hub, &mut rq, &mut context);

        let initial_children_count = editor.children.len();

        let mut bus = VecDeque::new();

        let handled = editor.handle_event(
            &Event::Select(EntryId::EditLibraryPath),
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
    fn test_file_chooser_closed_updates_path() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 600, 800];
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();

        let library = create_test_library();

        let mut editor = LibraryEditor::new(rect, 0, library, &hub, &mut rq, &mut context);

        let original_path = editor.library.path.clone();
        let new_path = std::path::PathBuf::from("/mnt/onboard/newpath");

        let mut bus = VecDeque::new();
        rq = RenderQueue::new();

        let handled = editor.handle_event(
            &Event::FileChooserClosed(Some(new_path.clone())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(!handled);
        assert_ne!(editor.library.path, original_path);
        assert_eq!(editor.library.path, new_path);
        assert!(rq.is_empty());
    }

    #[test]
    fn test_submit_library_name_updates_library() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 600, 800];
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();

        let library = create_test_library();

        let mut editor = LibraryEditor::new(rect, 0, library, &hub, &mut rq, &mut context);

        let original_name = editor.library.name.clone();
        let new_name = "Updated Library Name".to_string();

        let mut bus = VecDeque::new();
        rq = RenderQueue::new();

        let handled = editor.handle_event(
            &Event::Submit(ViewId::LibraryRenameInput, new_name.clone()),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(!handled);
        assert_ne!(editor.library.name, original_name);
        assert_eq!(editor.library.name, new_name);
        assert!(rq.is_empty());
    }
}
