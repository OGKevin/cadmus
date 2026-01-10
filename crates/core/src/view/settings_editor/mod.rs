use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, Rectangle};
use crate::helpers::save_toml;
use crate::settings::SETTINGS_PATH;
use crate::unit::scale_by_dpi;
use crate::view::common::{locate_by_id, toggle_main_menu};
use crate::view::filler::Filler;
use crate::view::icon::Icon;
use crate::view::top_bar::TopBar;
use crate::view::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, ID_FEEDER};
use crate::view::{BIG_BAR_HEIGHT, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use anyhow::Error;

mod category;
mod category_editor;
mod category_row;
mod setting_row;
mod setting_value;

pub use self::category::Category;
pub use self::category_editor::CategoryEditor;
pub use self::category_row::CategoryRow;
pub use self::setting_row::{Kind as RowKind, SettingRow};
pub use self::setting_value::SettingValue;

pub struct SettingsEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
}

pub struct SettingsEditorBuilder<'a> {
    rect: Rectangle,
    rq: &'a mut RenderQueue,
    context: &'a mut Context,
}

impl<'a> SettingsEditorBuilder<'a> {
    fn new(rect: Rectangle, rq: &'a mut RenderQueue, context: &'a mut Context) -> Self {
        SettingsEditorBuilder { rect, rq, context }
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
            "Settings".to_string(),
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

    fn build_category_row(&mut self, category: Category, row_rect: Rectangle) -> Box<dyn View> {
        let category_row = CategoryRow::new(category, row_rect, self.context);
        Box::new(category_row) as Box<dyn View>
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
        let bottom_bar_rect = rect![
            self.rect.min.x,
            self.rect.max.y - bar_height + separator_bottom_half,
            self.rect.max.x,
            self.rect.max.y
        ];

        let back_icon = Icon::new("back", bottom_bar_rect, Event::Back);
        Box::new(back_icon) as Box<dyn View>
    }

    pub fn build(mut self) -> Result<SettingsEditor, Error> {
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
        let categories = Category::all();
        let mut current_y = content_rect.min.y;

        for category in categories {
            let row_rect = rect![
                content_rect.min.x,
                current_y,
                content_rect.max.x,
                current_y + row_height
            ];
            children.push(self.build_category_row(category, row_rect));
            current_y += row_height;
        }

        children.push(self.build_bottom_separator(
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        children.push(self.build_bottom_bar(bar_height, separator_bottom_half));

        self.rq.add(RenderData::new(id, self.rect, UpdateMode::Gui));

        Ok(SettingsEditor {
            id,
            rect: self.rect,
            children,
        })
    }
}

impl SettingsEditor {
    pub fn new<'a>(
        rect: Rectangle,
        _hub: &'a Hub,
        rq: &'a mut RenderQueue,
        context: &'a mut Context,
    ) -> SettingsEditorBuilder<'a> {
        SettingsEditorBuilder::new(rect, rq, context)
    }
}

impl View for SettingsEditor {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::OpenSettingsCategory(category) => {
                let category_editor =
                    CategoryEditor::new(self.rect, *category, hub, rq, context).build();

                if let Ok(editor) = category_editor {
                    self.children.push(Box::new(editor));
                }

                true
            }
            Event::UpdateSettings(ref settings) => {
                context.settings = settings.clone();

                if let Err(e) = save_toml(&context.settings, SETTINGS_PATH) {
                    eprintln!("Failed to save settings: {:#}", e);
                    hub.send(Event::Notify("Failed to save settings".to_string()))
                        .ok();
                } else {
                    hub.send(Event::Notify("Settings saved successfully".to_string()))
                        .ok();
                }

                true
            }
            Event::ToggleNear(ViewId::MainMenu, rect) => {
                toggle_main_menu(self, *rect, None, rq, context);
                true
            }
            Event::Close(ViewId::MainMenu) => {
                toggle_main_menu(self, Rectangle::default(), Some(false), rq, context);
                true
            }
            Event::Close(view_id) => match view_id {
                ViewId::SettingsCategoryEditor => {
                    if let Some(index) = locate_by_id(self, *view_id) {
                        self.children.remove(index);
                        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    }
                    true
                }
                ViewId::MainMenu => {
                    toggle_main_menu(self, Rectangle::default(), Some(false), rq, context);
                    true
                }
                _ => return false,
            },
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
}
