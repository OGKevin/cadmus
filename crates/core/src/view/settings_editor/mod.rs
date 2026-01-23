//! Settings editor module for managing application configuration.
//!
//! This module provides a hierarchical settings interface with the following structure:
//!
//! ```text
//! SettingsEditor (Main view)
//!   └── CategoryRow (One for each category: General, Libraries, Intermissions)
//!       └── CategoryEditor (Opened when a category is selected)
//!           └── SettingRow (One for each setting in the category)
//!               ├── Label (Setting name)
//!               └── SettingValue (Current value, can be tapped to edit)
//! ```
//!
//! ## Components
//!
//! - **SettingsEditor**: Top-level view showing all setting categories
//! - **CategoryRow**: Represents a category in the settings list
//! - **CategoryEditor**: Full-screen editor for a specific category's settings
//! - **SettingRow**: Individual setting with label and value
//! - **SettingValue**: Interactive value display that opens editors/menus
//! - **LibraryEditor**: Specialized editor for library settings
//!
//! ## Event Flow
//!
//! When a setting is modified, the CategoryEditor updates its internal settings copy.
//! Changes are only persisted when the user taps the validate button, which sends
//! an `Event::UpdateSettings` to save the configuration.
//!
//! The grandchild update pattern is used to propagate setting changes from the
//! CategoryEditor to SettingValue views for UI updates without full rebuilds.

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
use crate::view::top_bar::{TopBar, TopBarVariant};
use crate::view::{
    Bus, Event, Hub, Id, NotificationEvent, RenderData, RenderQueue, View, ViewId, ID_FEEDER,
};
use crate::view::{BIG_BAR_HEIGHT, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};

mod bottom_bar;
mod category;
mod category_editor;
mod category_row;
mod library_editor;
mod setting_row;
mod setting_value;

pub use self::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};
pub use self::category::Category;
pub use self::category_editor::CategoryEditor;
pub use self::category_row::CategoryRow;
pub use self::setting_row::{Kind as RowKind, SettingRow};
pub use self::setting_value::SettingValue;

/// Main settings editor view.
///
/// This is the top-level view that displays all available setting categories
/// (General, Libraries, Intermissions) as interactive rows. When a category is
/// selected, it opens a full-screen `CategoryEditor` to allow editing of that
/// category's settings.
///
/// # Structure
///
/// - `id`: Unique identifier for this view
/// - `rect`: Bounding rectangle for the entire settings editor
/// - `children`: Child views including the top bar, separators, background, and category rows
pub struct SettingsEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
}

impl SettingsEditor {
    pub fn new(rect: Rectangle, rq: &mut RenderQueue, context: &mut Context) -> Self {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        let (bar_height, _separator_thickness, separator_top_half, separator_bottom_half) =
            Self::calculate_dimensions();

        children.push(Self::build_top_bar(
            &rect,
            bar_height,
            separator_top_half,
            context,
        ));

        children.push(Self::build_top_separator(
            &rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        let (background, content_rect) =
            Self::build_content_background(&rect, bar_height, separator_bottom_half);
        children.push(background);

        Self::build_category_rows(&mut children, &content_rect, context);

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        SettingsEditor { id, rect, children }
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
        rect: &Rectangle,
        bar_height: i32,
        separator_top_half: i32,
        context: &mut Context,
    ) -> Box<dyn View> {
        let top_bar = TopBar::new(
            rect![
                rect.min.x,
                rect.min.y,
                rect.max.x,
                rect.min.y + bar_height - separator_top_half
            ],
            TopBarVariant::Back,
            "Settings".to_string(),
            context,
        );
        Box::new(top_bar) as Box<dyn View>
    }

    fn build_top_separator(
        rect: &Rectangle,
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
        rect: &Rectangle,
        bar_height: i32,
        separator_bottom_half: i32,
    ) -> (Box<dyn View>, Rectangle) {
        let content_rect = rect![
            rect.min.x,
            rect.min.y + bar_height + separator_bottom_half,
            rect.max.x,
            rect.max.y
        ];

        let background = Filler::new(content_rect, WHITE);
        (Box::new(background) as Box<dyn View>, content_rect)
    }

    fn build_category_rows(
        children: &mut Vec<Box<dyn View>>,
        content_rect: &Rectangle,
        context: &mut Context,
    ) {
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
            let category_row = CategoryRow::new(category, row_rect, context);
            children.push(Box::new(category_row) as Box<dyn View>);
            current_y += row_height;
        }
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
                let category_editor = CategoryEditor::new(self.rect, *category, hub, rq, context);

                if let Ok(editor) = category_editor {
                    self.children.push(Box::new(editor));
                }

                true
            }
            Event::UpdateSettings(ref settings) => {
                context.settings = (**settings).clone();

                if let Err(e) = save_toml(&context.settings, SETTINGS_PATH) {
                    eprintln!("Failed to save settings: {:#}", e);
                    hub.send(Event::Notification(NotificationEvent::Show(
                        "Failed to save settings".to_string(),
                    )))
                    .ok();
                } else {
                    hub.send(Event::Notification(NotificationEvent::Show(
                        "Settings saved successfully".to_string(),
                    )))
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
                _ => false,
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
