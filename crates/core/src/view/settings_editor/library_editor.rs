use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::Fonts;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, Rectangle};
use crate::settings::{LibrarySettings, Settings};
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::file_chooser::{FileChooser, SelectionMode};
use crate::view::filler::Filler;
use crate::view::icon::Icon;
use crate::view::menu::Menu;
use crate::view::named_input::NamedInput;
use crate::view::toggleable_keyboard::ToggleableKeyboard;
use crate::view::top_bar::TopBar;
use crate::view::EntryId;
use crate::view::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, ID_FEEDER};
use crate::view::{BIG_BAR_HEIGHT, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use anyhow::Error;

use super::setting_row::{Kind as RowKind, SettingRow};

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

pub struct LibraryEditorBuilder<'a> {
    rect: Rectangle,
    library_index: usize,
    library: LibrarySettings,
    rq: &'a mut RenderQueue,
    context: &'a mut Context,
    settings: Settings,
}

impl<'a> LibraryEditorBuilder<'a> {
    fn new(
        rect: Rectangle,
        library_index: usize,
        library: LibrarySettings,
        rq: &'a mut RenderQueue,
        context: &'a mut Context,
    ) -> Self {
        let mut settings = context.settings.clone();
        if library_index < settings.libraries.len() {
            settings.libraries[library_index] = library.clone();
        }

        LibraryEditorBuilder {
            rect,
            library_index,
            library,
            rq,
            context,
            settings,
        }
    }

    fn calculate_dimensions(&self) -> (i32, i32, i32, i32) {
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

    fn build_top_bar(&mut self, bar_height: i32, separator_top_half: i32) -> Box<dyn View> {
        let top_bar = TopBar::new(
            rect![
                self.rect.min.x,
                self.rect.min.y,
                self.rect.max.x,
                self.rect.min.y + bar_height - separator_top_half
            ],
            Event::Back,
            "Library Editor".to_string(),
            self.context,
        );
        Box::new(top_bar) as Box<dyn View>
    }

    fn build_top_separator(
        &mut self,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        let separator = Filler::new(
            rect![
                self.rect.min.x,
                self.rect.min.y + bar_height - separator_top_half,
                self.rect.max.x,
                self.rect.min.y + bar_height + separator_bottom_half
            ],
            BLACK,
        );
        Box::new(separator) as Box<dyn View>
    }

    fn build_content_rows(
        &mut self,
        bar_height: i32,
        separator_thickness: i32,
    ) -> Vec<Box<dyn View>> {
        let mut children = Vec::new();
        let dpi = CURRENT_DEVICE.dpi;
        let row_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;

        let content_start_y = self.rect.min.y + bar_height + separator_thickness;
        let content_end_y = self.rect.max.y - bar_height - separator_thickness;

        let mut current_y = content_start_y;

        if current_y + row_height <= content_end_y {
            let name_row_rect = rect![
                self.rect.min.x,
                current_y,
                self.rect.max.x,
                current_y + row_height
            ];
            children.push(self.build_name_row(name_row_rect));
            current_y += row_height;
        }

        if current_y + row_height <= content_end_y {
            let path_row_rect = rect![
                self.rect.min.x,
                current_y,
                self.rect.max.x,
                current_y + row_height
            ];
            children.push(self.build_path_row(path_row_rect));
            current_y += row_height;
        }

        if current_y + row_height <= content_end_y {
            let mode_row_rect = rect![
                self.rect.min.x,
                current_y,
                self.rect.max.x,
                current_y + row_height
            ];
            children.push(self.build_mode_row(mode_row_rect));
        }

        children
    }

    fn build_name_row(&mut self, rect: Rectangle) -> Box<dyn View> {
        Box::new(SettingRow::new(
            RowKind::LibraryName(self.library_index),
            rect,
            &self.settings,
        )) as Box<dyn View>
    }

    fn build_path_row(&mut self, rect: Rectangle) -> Box<dyn View> {
        Box::new(SettingRow::new(
            RowKind::LibraryPath(self.library_index),
            rect,
            &self.settings,
        )) as Box<dyn View>
    }

    fn build_mode_row(&mut self, rect: Rectangle) -> Box<dyn View> {
        Box::new(SettingRow::new(
            RowKind::LibraryMode(self.library_index),
            rect,
            &self.settings,
        )) as Box<dyn View>
    }

    fn build_bottom_separator(
        &mut self,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        let separator = Filler::new(
            rect![
                self.rect.min.x,
                self.rect.max.y - bar_height - separator_top_half,
                self.rect.max.x,
                self.rect.max.y - bar_height + separator_bottom_half
            ],
            BLACK,
        );
        Box::new(separator) as Box<dyn View>
    }

    fn build_bottom_bar(&mut self, bar_height: i32, separator_bottom_half: i32) -> Box<dyn View> {
        let dpi = CURRENT_DEVICE.dpi;
        let button_width = scale_by_dpi(120.0, dpi) as i32;
        let padding = scale_by_dpi(10.0, dpi) as i32;

        let bottom_bar_rect = rect![
            self.rect.min.x,
            self.rect.max.y - bar_height + separator_bottom_half,
            self.rect.max.x,
            self.rect.max.y
        ];

        let mut bottom_bar_children: Vec<Box<dyn View>> = Vec::new();

        let cancel_rect = rect![
            bottom_bar_rect.min.x + padding,
            bottom_bar_rect.min.y,
            bottom_bar_rect.min.x + padding + button_width,
            bottom_bar_rect.max.y
        ];
        let cancel_icon = Icon::new("close", cancel_rect, Event::Close(ViewId::LibraryEditor));
        bottom_bar_children.push(Box::new(cancel_icon) as Box<dyn View>);

        let save_rect = rect![
            bottom_bar_rect.max.x - padding - button_width,
            bottom_bar_rect.min.y,
            bottom_bar_rect.max.x - padding,
            bottom_bar_rect.max.y
        ];
        let save_icon = Icon::new("check_mark-large", save_rect, Event::Validate);
        bottom_bar_children.push(Box::new(save_icon) as Box<dyn View>);

        let filler = Filler::new(bottom_bar_rect, WHITE);
        let mut bottom_bar = Box::new(filler);
        *bottom_bar.children_mut() = bottom_bar_children;

        bottom_bar
    }

    pub fn build(mut self) -> Result<LibraryEditor, Error> {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        children.push(Box::new(Filler::new(self.rect, WHITE)) as Box<dyn View>);

        let (bar_height, separator_thickness, separator_top_half, separator_bottom_half) =
            self.calculate_dimensions();

        children.push(self.build_top_bar(bar_height, separator_top_half));
        children.push(self.build_top_separator(
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        children.extend(self.build_content_rows(bar_height, separator_thickness));

        children.push(self.build_bottom_separator(
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));
        children.push(self.build_bottom_bar(bar_height, separator_bottom_half));

        let keyboard = ToggleableKeyboard::new(self.rect, false);
        children.push(Box::new(keyboard) as Box<dyn View>);

        let keyboard_index = children.len() - 1;

        self.rq.add(RenderData::new(id, self.rect, UpdateMode::Gui));

        Ok(LibraryEditor {
            id,
            rect: self.rect,
            children,
            library_index: self.library_index,
            library: self.library.clone(),
            _original_library: self.library,
            focus: None,
            keyboard_index,
        })
    }
}

impl LibraryEditor {
    pub fn new(
        rect: Rectangle,
        library_index: usize,
        library: LibrarySettings,
        _hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> Result<LibraryEditor, Error> {
        let builder = LibraryEditorBuilder::new(rect, library_index, library, rq, context);
        builder.build()
    }

    fn update_row_value(&mut self, rq: &mut RenderQueue) {
        // Event propagation via UpdateLibrary will handle updating the SettingValue widgets
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
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
}

impl View for LibraryEditor {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Focus(v) => {
                if self.focus != v {
                    self.focus = v;
                    if v.is_some() {
                        self.toggle_keyboard(true, v, hub, rq, context);
                    } else {
                        self.toggle_keyboard(false, None, hub, rq, context);
                    }
                }
                true
            }
            Event::Validate => {
                bus.push_back(Event::UpdateLibrary(
                    self.library_index,
                    Box::new(self.library.clone()),
                ));
                bus.push_back(Event::Close(ViewId::LibraryEditor));

                true
            }
            Event::Select(EntryId::EditLibraryName) => {
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
            Event::Select(EntryId::EditLibraryPath) => {
                let file_chooser = FileChooser::new(
                    self.rect,
                    self.library.path.clone(),
                    SelectionMode::Directory,
                    hub,
                    rq,
                    context,
                );
                self.children.push(Box::new(file_chooser));
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                true
            }
            Event::Select(EntryId::SetLibraryMode(mode)) => {
                self.library.mode = mode;
                self.update_row_value(rq);
                false
            }
            Event::Submit(ViewId::LibraryRenameInput, ref text) => {
                self.library.name = text.clone();
                self.update_row_value(rq);

                false
            }
            Event::FileChooserClosed(ref path) => {
                if let Some(path) = path {
                    self.library.path = path.clone();
                    self.update_row_value(rq);
                }

                false
            }
            Event::SubMenu(rect, ref entries) => {
                let menu = Menu::new(
                    rect,
                    ViewId::SettingsValueMenu,
                    crate::view::menu::MenuKind::SubMenu,
                    entries.clone(),
                    context,
                );
                rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
                self.children.push(Box::new(menu));
                true
            }
            Event::Close(view) => match view {
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
                        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    }

                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

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
