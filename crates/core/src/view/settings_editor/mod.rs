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
//!
//! ## Adding a new setting
//!
//! Each setting is a small self-contained struct that implements [`SettingKind`].
//! The trait carries everything the UI needs.
//!
//! ### 1. Add a variant to `SettingIdentity`
//!
//! Open `kinds/identity.rs` and add a variant for the new setting:
//!
//! ```rust,no_run
//! pub enum SettingIdentity {
//!     // ... existing variants
//!     MyNewSetting,
//! }
//! ```
//!
//! ### 2. Implement `SettingKind`
//!
//! Add a struct in the appropriate `kinds/*.rs` file and implement the trait:
//!
//! ```rust,ignore
//! // This example uses hypothetical types (MyNewSetting, EntryId::EditMyNewSetting)
//! // that do not exist in the codebase — it illustrates the pattern to follow.
//! pub struct MyNewSetting;
//!
//! impl SettingKind for MyNewSetting {
//!     fn identity(&self) -> SettingIdentity {
//!         SettingIdentity::MyNewSetting
//!     }
//!
//!     fn label(&self, _settings: &Settings) -> String {
//!         "My New Setting".to_string()
//!     }
//!
//!     fn fetch(&self, settings: &Settings) -> SettingData {
//!         SettingData {
//!             value: settings.my_new_setting.to_string(),
//!             widget: WidgetKind::ActionLabel(Event::Select(EntryId::EditMyNewSetting)),
//!         }
//!     }
//! }
//! ```
//!
//! For a **toggle** widget, use `WidgetKind::Toggle { ..., tap_event }` where
//! `tap_event` is `Event::Toggle(ToggleEvent::Setting(ToggleSettings::MyNewSetting))`
//! (adding the corresponding `ToggleSettings` variant in `kinds/mod.rs`).
//!
//! For a **sub-menu** (radio buttons), use `WidgetKind::SubMenu(entries)` where
//! `entries` is a `Vec<`[`EntryKind`](crate::view::EntryKind)`>` — the sub-menu
//! event is built automatically from the entries when the row is tapped.
//!
//! ### 3. Register the setting in `Category::settings()`
//!
//! Open `category.rs` and add the new kind to the relevant category arm:
//!
//! ```rust,ignore
//! // This example shows a match arm inside Category::settings() — it is a
//! // partial snippet and cannot compile standalone.
//! Category::General => vec![
//!     // ... existing kinds
//!     Box::new(MyNewSetting),
//! ],
//! ```
//!
//! ### 4. Handle mutations (usually automatic)
//!
//! Most settings do **not** require any changes to `CategoryEditor`. The framework
//! handles mutations automatically:
//!
//! - **Sub-menu / toggle / file-chooser settings**: [`SettingKind::handle`] is called
//!   when the user makes a selection. Implement `handle` on your struct to mutate
//!   `context.settings` and return the updated display string.
//! - **Text-input settings**: Implement [`InputSettingKind`] and its `apply_text`
//!   method. The overlay and submission flow are handled by [`SettingValue`].
//!
//! A `CategoryEditor` handler is only needed for settings with **custom event flows**
//! not covered by the traits above — for example, library management actions that
//! must coordinate multiple views or emit side-effect events. In that case, add a
//! handler method in `category_editor.rs` and dispatch it from `handle_event`. After
//! mutating `context.settings`, send `SettingsEvent::UpdateValue` so the corresponding
//! [`SettingValue`] view refreshes its displayed text:
//!
//! ```rust,ignore
//! // This example shows a method inside a CategoryEditor impl block — it is a
//! // partial snippet and cannot compile standalone.
//! fn handle_my_new_setting(&mut self, value: f32, hub: &Hub, context: &mut Context) -> bool {
//!     context.settings.my_new_setting = value;
//!     hub.send(Event::Settings(SettingsEvent::UpdateValue {
//!         kind: SettingIdentity::MyNewSetting,
//!         value: value.to_string(),
//!     }))
//!     .ok();
//!     true
//! }
//! ```

use crate::color::{BLACK, SEPARATOR_NORMAL};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{Rectangle, halves};
use crate::unit::scale_by_dpi;
use crate::view::common::toggle_main_menu;
use crate::view::filler::Filler;
use crate::view::navigation::stack_navigation_bar::StackNavigationBar;
use crate::view::top_bar::{TopBar, TopBarVariant};
use crate::view::{Bus, Event, Hub, ID_FEEDER, Id, RenderData, RenderQueue, View, ViewId};
use crate::view::{SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use fxhash::FxHashMap;

pub mod kinds;

mod bottom_bar;
mod category;
mod category_button;
mod category_editor;
mod category_navigation_bar;
mod category_provider;
mod editor_utils;
mod library_editor;
mod refresh_rate_by_kind_editor;
mod setting_row;
mod setting_value;

pub use setting_value::{SettingsEvent, ToggleSettings};

pub use self::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};
pub use self::category::Category;
pub use self::category_button::CategoryButton;
pub use self::category_editor::CategoryEditor;
pub use self::category_navigation_bar::CategoryNavigationBar;
pub use self::category_provider::SettingsCategoryProvider;
pub use self::kinds::{InputSettingKind, SettingIdentity, SettingKind};
pub use self::setting_row::SettingRow;
pub use self::setting_value::SettingValue;

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
/// - `editors`: Pre-built [`CategoryEditor`] instances for all inactive categories, keyed by
///   [`Category`]. On tab switch the active editor is returned here and the target is pulled
///   out, avoiding a full view-tree rebuild on every navigation. The [`Category::Libraries`]
///   editor is included and stays current because every library mutation calls
///   `rebuild_library_rows` before returning.
pub struct SettingsEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    nav_bar_index: usize,
    editor_index: usize,
    editors: FxHashMap<Category, Box<dyn View>>,
}

impl SettingsEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(rq, context)))]
    pub fn new(rect: Rectangle, rq: &mut RenderQueue, context: &mut Context) -> Self {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        let (bar_height, separator_thickness, separator_top_half, separator_bottom_half) =
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

        let provider = SettingsCategoryProvider;
        let mut navigation_bar =
            StackNavigationBar::new(nav_bar_rect, rect.max.y, 2, provider, Category::General)
                .disable_resize();

        navigation_bar.set_selected(Category::General, rq, context);
        let nav_bar_index = children.len();
        children.push(Box::new(navigation_bar));

        let nav_bar_max_y = children[nav_bar_index].rect().max.y;
        let (sep_top_half, sep_bottom_half) = halves(separator_thickness);
        children.push(Self::build_nav_bar_separator(
            &rect,
            nav_bar_max_y,
            sep_top_half,
            sep_bottom_half,
        ));

        let content_rect = rect![
            rect.min.x,
            nav_bar_max_y + sep_bottom_half,
            rect.max.x,
            rect.max.y
        ];

        let category_editor = CategoryEditor::new(content_rect, Category::General, rq, context);

        let editor_index = children.len();
        children.push(Box::new(category_editor));

        let mut editors: FxHashMap<Category, Box<dyn View>> = FxHashMap::default();

        for category in Category::all() {
            if category == Category::General {
                continue;
            }

            let editor = CategoryEditor::new(content_rect, category, rq, context);
            editors.insert(category, Box::new(editor));
        }

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        SettingsEditor {
            id,
            rect,
            children,
            nav_bar_index,
            editor_index,
            editors,
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

    fn build_nav_bar_separator(
        rect: &Rectangle,
        nav_bar_max_y: i32,
        sep_top_half: i32,
        sep_bottom_half: i32,
    ) -> Box<dyn View> {
        let separator = Filler::new(
            rect![
                rect.min.x,
                nav_bar_max_y - sep_top_half,
                rect.max.x,
                nav_bar_max_y + sep_bottom_half
            ],
            SEPARATOR_NORMAL,
        );
        Box::new(separator) as Box<dyn View>
    }
}

impl View for SettingsEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, _bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
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

                let (_, separator_thickness, _, _) = Self::calculate_dimensions();
                let (sep_top_half, sep_bottom_half) = halves(separator_thickness);
                let sep_index = self.nav_bar_index + 1;
                *self.children[sep_index].rect_mut() = rect![
                    self.rect.min.x,
                    nav_bar_max_y - sep_top_half,
                    self.rect.max.x,
                    nav_bar_max_y + sep_bottom_half
                ];

                let current_category = self.children[self.editor_index]
                    .downcast_ref::<CategoryEditor>()
                    .map(|e| e.category());

                let outgoing = self.children.remove(self.editor_index);

                if let Some(cat) = current_category {
                    self.editors.insert(cat, outgoing);
                }

                let content_rect = rect![
                    self.rect.min.x,
                    nav_bar_max_y + sep_bottom_half,
                    self.rect.max.x,
                    self.rect.max.y
                ];

                let incoming = self.editors.remove(category).unwrap_or_else(|| {
                    Box::new(CategoryEditor::new(content_rect, *category, rq, context))
                        as Box<dyn View>
                });

                self.children.insert(self.editor_index, incoming);

                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                true
            }
            Event::NavigationBarResized(_) => {
                unimplemented!(
                    "The settings navigation bar should not be resizable which means this event is not expected to be send."
                )
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
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
