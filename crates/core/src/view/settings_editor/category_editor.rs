use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, Rectangle};
use crate::settings::Settings;
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::filler::Filler;
use crate::view::icon::Icon;
use crate::view::menu::Menu;
use crate::view::top_bar::TopBar;
use crate::view::{
    Bus, EntryId, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, BIG_BAR_HEIGHT, ID_FEEDER,
    SMALL_BAR_HEIGHT, THICKNESS_MEDIUM,
};
use anyhow::Error;

use super::category::Category;
use super::library_editor::LibraryEditor;
use super::setting_row::{Kind as RowKind, SettingRow};

pub struct CategoryEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    #[allow(dead_code)]
    category: Category,
    settings: Settings,
    original_settings: Settings,
    content_rect: Rectangle,
    row_height: i32,
}

pub struct CategoryEditorBuilder<'a> {
    rect: Rectangle,
    category: Category,
    rq: &'a mut RenderQueue,
    context: &'a mut Context,
    settings: Settings,
}

impl<'a> CategoryEditorBuilder<'a> {
    fn new(
        rect: Rectangle,
        category: Category,
        rq: &'a mut RenderQueue,
        context: &'a mut Context,
    ) -> Self {
        CategoryEditorBuilder {
            rect,
            category,
            rq,
            settings: context.settings.clone(),
            context,
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
            self.category.label(),
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

    fn build_content_background(
        &mut self,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> (Box<dyn View>, Rectangle) {
        let content_rect = rect![
            self.rect.min.x,
            self.rect.min.y + bar_height + separator_bottom_half,
            self.rect.max.x,
            self.rect.max.y - bar_height - separator_top_half
        ];

        let background = Filler::new(content_rect, WHITE);
        (Box::new(background) as Box<dyn View>, content_rect)
    }

    fn build_setting_row(
        &mut self,
        kind: RowKind,
        row_rect: Rectangle,
    ) -> Result<Box<dyn View>, Error> {
        let setting_row = SettingRow::new(kind, row_rect, &self.settings);
        Ok(Box::new(setting_row) as Box<dyn View>)
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

    fn build_bottom_bar(
        &mut self,
        bar_height: i32,
        separator_bottom_half: i32,
    ) -> (Box<dyn View>, Box<dyn View>) {
        let bottom_bar_rect = rect![
            self.rect.min.x,
            self.rect.max.y - bar_height + separator_bottom_half,
            self.rect.max.x,
            self.rect.max.y
        ];

        let button_width = bottom_bar_rect.width() as i32 / 2;

        let cancel_rect = rect![
            bottom_bar_rect.min.x,
            bottom_bar_rect.min.y,
            bottom_bar_rect.min.x + button_width,
            bottom_bar_rect.max.y
        ];

        let cancel_icon = Icon::new(
            "back",
            cancel_rect,
            Event::Close(ViewId::SettingsCategoryEditor),
        );

        let save_rect = rect![
            bottom_bar_rect.min.x + button_width,
            bottom_bar_rect.min.y,
            bottom_bar_rect.max.x,
            bottom_bar_rect.max.y
        ];

        let save_icon = Icon::new("check_mark", save_rect, Event::Validate);

        (
            Box::new(cancel_icon) as Box<dyn View>,
            Box::new(save_icon) as Box<dyn View>,
        )
    }

    pub fn build(mut self) -> Result<CategoryEditor, Error> {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        let (bar_height, _separator_thickness, separator_top_half, separator_bottom_half) =
            self.calculate_dimensions();

        children.push(self.build_top_bar(bar_height, separator_top_half));

        children.push(self.build_top_separator(
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        let (background, content_rect) =
            self.build_content_background(bar_height, separator_top_half, separator_bottom_half);
        children.push(background);

        let row_height = scale_by_dpi(BIG_BAR_HEIGHT, CURRENT_DEVICE.dpi) as i32;
        let setting_kinds = self.category.settings(self.context);
        let mut current_y = content_rect.min.y;

        for kind in setting_kinds {
            let row_rect = rect![
                content_rect.min.x,
                current_y,
                content_rect.max.x,
                current_y + row_height
            ];
            children.push(self.build_setting_row(kind, row_rect)?);
            current_y += row_height;
        }

        children.push(self.build_bottom_separator(
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        let (cancel_icon, save_icon) = self.build_bottom_bar(bar_height, separator_bottom_half);
        children.push(cancel_icon);
        children.push(save_icon);

        self.rq.add(RenderData::new(id, self.rect, UpdateMode::Gui));

        Ok(CategoryEditor {
            id,
            rect: self.rect,
            children,
            category: self.category,
            original_settings: self.settings.clone(),
            settings: self.settings,
            content_rect,
            row_height,
        })
    }
}

impl CategoryEditor {
    pub fn new<'a>(
        rect: Rectangle,
        category: Category,
        _hub: &'a Hub,
        rq: &'a mut RenderQueue,
        context: &'a mut Context,
    ) -> CategoryEditorBuilder<'a> {
        CategoryEditorBuilder::new(rect, category, rq, context)
    }

    fn rebuild_library_rows(&mut self, rq: &mut RenderQueue) -> Result<(), Error> {
        if self.category != Category::Libraries {
            return Ok(());
        }

        let num_libraries = self.settings.libraries.len();

        let first_row_index = 3;

        for _ in 0..num_libraries {
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
            Event::Back => {
                bus.push_back(Event::Close(ViewId::SettingsCategoryEditor));
                true
            }
            Event::SubMenu(rect, ref entries) => {
                let menu = Menu::new(
                    *rect,
                    ViewId::SettingsValueMenu,
                    crate::view::menu::MenuKind::SubMenu,
                    entries.clone(),
                    context,
                );

                rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
                self.children.push(Box::new(menu));

                true
            }
            Event::Select(ref id) => match id {
                EntryId::SetKeyboardLayout(ref layout) => {
                    self.settings.keyboard_layout = layout.clone();
                    false
                }
                EntryId::ToggleSleepCover => {
                    self.settings.sleep_cover = !self.settings.sleep_cover;
                    false
                }
                EntryId::ToggleAutoShare => {
                    self.settings.auto_share = !self.settings.auto_share;
                    false
                }
                EntryId::SetButtonScheme(button_scheme) => {
                    self.settings.button_scheme = *button_scheme;
                    false
                }
                _ => false,
            },
            Event::Validate => {
                bus.push_back(Event::UpdateSettings(self.settings.clone()));
                bus.push_back(Event::Close(ViewId::SettingsCategoryEditor));
                true
            }
            Event::EditLibrary(index) => {
                if let Some(library) = self.settings.libraries.get(*index).cloned() {
                    if let Ok(library_editor) =
                        LibraryEditor::new(self.rect, *index, library, hub, rq, context)
                    {
                        self.children.push(Box::new(library_editor));
                        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    }
                }
                true
            }
            Event::UpdateLibrary(index, ref library) => {
                if *index < self.settings.libraries.len() {
                    self.settings.libraries[*index] = (**library).clone();

                    if let Err(e) = self.rebuild_library_rows(rq) {
                        eprintln!("Failed to rebuild library rows: {}", e);
                    }
                }

                false
            }
            Event::Close(ViewId::LibraryEditor) => {
                if let Some(index) = locate_by_id(self, ViewId::LibraryEditor) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            Event::Close(view_id) if *view_id == ViewId::SettingsCategoryEditor => {
                self.settings = self.original_settings.clone();
                false
            }
            Event::Close(view_id) if *view_id == ViewId::SettingsValueMenu => {
                if let Some(index) = locate_by_id(self, *view_id) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }

                true
            }
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
