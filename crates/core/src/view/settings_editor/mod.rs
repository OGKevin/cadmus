//! Settings editor module for managing application configuration.
//!
//! This module provides a hierarchical settings interface with the following structure:
//!
//! ```text
//! SettingsEditor (Main view)
//!   ├── TopBar (Back button, "Settings" title)
//!   ├── StackNavigationBar (Category tabs: General | Libraries | Intermissions)
//!   └── CategoryEditor (Embedded, shows settings for selected category)
//!       ├── SettingRow (One for each setting in the category)
//!       │   ├── Label (Setting name)
//!       │   └── SettingValue (Current value, can be tapped to edit)
//!       └── BottomBar (Add Library button for Libraries category)
//! ```
//!
//! ## Components
//!
//! - **SettingsEditor**: Top-level view with navigation bar and category editor
//! - **CategoryNavigationBar**: Horizontal bar with category tabs
//! - **CategoryEditor**: Embedded editor for a specific category's settings
//! - **SettingRow**: Individual setting with label and value
//! - **SettingValue**: Interactive value display that opens editors/menus
//! - **LibraryEditor**: Specialized editor for library settings
//!
//! ## Event Flow
//!
//! When a setting is modified, the CategoryEditor directly updates `context.settings`,
//! providing immediate feedback. Settings are persisted to disk when the settings editor
//! is closed.

use crate::color::BLACK;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, Rectangle};
use crate::unit::scale_by_dpi;
use crate::view::common::toggle_main_menu;
use crate::view::filler::Filler;
use crate::view::navigation::stack_navigation_bar::StackNavigationBar;
use crate::view::top_bar::{TopBar, TopBarVariant};
use crate::view::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, ID_FEEDER};
use crate::view::{SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};

mod bottom_bar;
mod category;
mod category_button;
mod category_editor;
mod category_navigation_bar;
mod category_provider;
mod library_editor;
mod setting_row;
mod setting_value;

pub use setting_value::ToggleSettings;

pub use self::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};
pub use self::category::Category;
pub use self::category_button::CategoryButton;
pub use self::category_editor::CategoryEditor;
pub use self::category_navigation_bar::CategoryNavigationBar;
pub use self::category_provider::SettingsCategoryProvider;
pub use self::setting_row::{Kind as RowKind, SettingRow};
pub use self::setting_value::SettingValue;

// pub enum ToggleSettings{}

/// Main settings editor view.
///
/// This is the top-level view that displays a navigation bar with category tabs
/// and an embedded category editor below it. When a category tab is selected,
/// the editor switches to show that category's settings.
///
/// # Structure
///
/// - `id`: Unique identifier for this view
/// - `rect`: Bounding rectangle for the entire settings editor
/// - `children`: Child views including the top bar, separators, navigation bar, and category editor
/// - `nav_bar_index`: Index of the StackNavigationBar in the children vector
/// - `editor_index`: Index of the CategoryEditor in the children vector
pub struct SettingsEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    nav_bar_index: usize,
    editor_index: usize,
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

        let nav_bar_rect = rect![
            rect.min.x,
            rect.min.y + bar_height + separator_bottom_half,
            rect.max.x,
            rect.min.y + bar_height + separator_bottom_half + bar_height
        ];

        let provider = SettingsCategoryProvider::default();
        let mut navigation_bar =
            StackNavigationBar::new(nav_bar_rect, rect.max.y, 1, provider, Category::General)
                .disable_resize();

        navigation_bar.set_selected(Category::General, rq, context);
        let nav_bar_index = children.len();
        children.push(Box::new(navigation_bar));

        let content_rect = rect![
            rect.min.x,
            children[nav_bar_index].rect().max.y,
            rect.max.x,
            rect.max.y
        ];

        let category_editor = CategoryEditor::new(content_rect, Category::General, rq, context);

        let editor_index = children.len();
        children.push(Box::new(category_editor));

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        SettingsEditor {
            id,
            rect,
            children,
            nav_bar_index,
            editor_index,
        }
    }

    fn calculate_dimensions() -> (i32, i32, i32, i32) {
        let dpi = CURRENT_DEVICE.dpi;
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
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
}

impl View for SettingsEditor {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::FileChooserClosed(_) => {
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                true
            }
            Event::SelectSettingsCategory(category) => {
                let nav_bar_max_y = {
                    let nav_bar = self.children[self.nav_bar_index]
                        .downcast_mut::<StackNavigationBar<SettingsCategoryProvider>>()
                        .unwrap();

                    nav_bar.set_selected(*category, rq, context);
                    nav_bar.rect.max.y
                };

                self.children.remove(self.editor_index);

                let content_rect = rect![
                    self.rect.min.x,
                    nav_bar_max_y,
                    self.rect.max.x,
                    self.rect.max.y
                ];

                let new_editor = CategoryEditor::new(content_rect, *category, rq, context);
                self.children
                    .insert(self.editor_index, Box::new(new_editor));

                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

                true
            }
            Event::NavigationBarResized(_) => {
                unimplemented!("The settings navigation bar should not be resizable which means this event is not expected to be send.")
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
                ViewId::MainMenu => {
                    toggle_main_menu(self, Rectangle::default(), Some(false), rq, context);
                    true
                }
                ViewId::FileChooser => {
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

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
